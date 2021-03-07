use crate::{tls_get, tls_init, tls_set, CmdResult, FunResult};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::process::{Child, Command, ExitStatus, Stdio};

pub type CmdArgs = Vec<String>;
pub type CmdEnvs = HashMap<String, String>;
type FnFun = fn(CmdArgs, CmdEnvs) -> FunResult;

tls_init!(CMD_MAP, HashMap<&'static str, FnFun>, HashMap::new());

#[doc(hidden)]
pub fn export_cmd(cmd: &'static str, func: FnFun) {
    tls_set!(CMD_MAP, |map| map.insert(cmd, func));
}

#[doc(hidden)]
pub fn set_debug(enable: bool) {
    std::env::set_var("CMD_LIB_DEBUG", if enable { "1" } else { "0" });
}

#[doc(hidden)]
#[derive(Default)]
pub struct GroupCmds {
    cmds: Vec<(Cmds, Option<Cmds>)>, // (cmd, orCmd) pairs
}

impl GroupCmds {
    pub fn add(mut self, cmds: Cmds, or_cmds: Option<Cmds>) -> Self {
        self.cmds.push((cmds, or_cmds));
        self
    }

    pub fn run_cmd(self) -> CmdResult {
        for cmd in self.cmds.into_iter() {
            if let Err(err) = cmd.0.run_cmd() {
                if let Some(or_cmds) = cmd.1 {
                    or_cmds.run_cmd()?;
                } else {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    pub fn run_fun(self) -> FunResult {
        let mut ret = String::new();
        for cmd in self.cmds.into_iter() {
            let ret0 = cmd.0.run_fun();
            match ret0 {
                Err(e) => {
                    if let Some(or_cmds) = cmd.1 {
                        ret = or_cmds.run_fun()?;
                    } else {
                        return Err(e);
                    }
                }
                Ok(r) => ret = r,
            };
        }
        Ok(ret)
    }

    pub fn spawn(mut self) -> std::io::Result<Vec<Child>> {
        assert_eq!(self.cmds.len(), 1);
        self.cmds.pop().unwrap().0.spawn()
    }

    pub fn spawn_with_output(mut self) -> std::io::Result<Vec<Child>> {
        assert_eq!(self.cmds.len(), 1);
        self.cmds.pop().unwrap().0.spawn_with_output()
    }
}

#[doc(hidden)]
#[derive(Default)]
pub struct Cmds {
    pipes: Vec<Command>,
    children: Vec<Child>,

    cmd_args: Vec<Cmd>,
    full_cmd: String,

    current_dir: String,
}

pub trait WaitResult {
    fn wait_fun_result(&mut self) -> FunResult;
    fn wait_cmd_result(&mut self) -> CmdResult;
}

impl WaitResult for Vec<Child> {
    fn wait_fun_result(&mut self) -> FunResult {
        let mut ret = String::new();
        let len = self.len();
        for i in (0..len).rev() {
            if i == len - 1 {
                let output = self.pop().unwrap().wait_with_output()?;
                if !output.status.success() {
                    return Err(Cmds::to_io_error("wait error", output.status));
                } else {
                    ret = String::from_utf8_lossy(&output.stdout).to_string();
                    if ret.ends_with('\n') {
                        ret.pop();
                    }
                }
            } else {
                let status = self.pop().unwrap().wait()?;
                if !status.success() {
                    return Err(Cmds::to_io_error("child status error", status));
                }
            }
        }
        Ok(ret)
    }

    fn wait_cmd_result(&mut self) -> CmdResult {
        let len = self.len();
        for i in (0..len).rev() {
            if i == len - 1 {
                let status = self.pop().unwrap().wait()?;
                if !status.success() {
                    return Err(Cmds::to_io_error("child status error", status));
                }
            } else {
                let status = self.pop().unwrap().wait()?;
                if !status.success() {
                    return Err(Cmds::to_io_error("child status error", status));
                }
            }
        }
        Ok(())
    }
}

impl Cmds {
    pub fn from_cmd(mut cmd: Cmd) -> Self {
        let cmd_args: Vec<String> = cmd.get_args().to_vec();
        Self {
            pipes: vec![cmd.gen_command()],
            children: vec![],
            full_cmd: cmd_args.join(" ").to_string(),
            cmd_args: vec![cmd],
            current_dir: String::new(),
        }
    }

    pub fn pipe(mut self, mut cmd: Cmd) -> Self {
        if !self.pipes.is_empty() {
            let last_i = self.pipes.len() - 1;
            self.pipes[last_i].stdout(Stdio::piped());
        }

        let cmd_args: Vec<String> = cmd.get_args().to_vec();
        let mut pipe_cmd = cmd.gen_command();
        for (k, v) in cmd.get_envs() {
            pipe_cmd.env(k, v);
        }
        if !self.current_dir.is_empty() {
            pipe_cmd.current_dir(self.current_dir.clone());
        }
        self.pipes.push(pipe_cmd);

        if !self.full_cmd.is_empty() {
            self.full_cmd += " | ";
        }
        self.full_cmd += &cmd_args.join(" ");
        self.cmd_args.push(cmd);
        self
    }

    fn spawn_with_output(mut self) -> std::io::Result<Vec<Child>> {
        self.pipes.last_mut().unwrap().stdout(Stdio::piped());
        self.spawn()
    }

    fn spawn(mut self) -> std::io::Result<Vec<Child>> {
        if let Ok(debug) = std::env::var("CMD_LIB_DEBUG") {
            if debug == "1" {
                eprintln!("Running \"{}\" ...", self.full_cmd);
            }
        }

        // spawning all the sub-processes
        for (i, cmd) in self.pipes.iter_mut().enumerate() {
            if i != 0 {
                if let Some(output) = self.children[i - 1].stdout.take() {
                    cmd.stdin(output);
                }
            }
            self.children.push(cmd.spawn()?);
        }

        Ok(self.children)
    }

    fn run_cd_cmd(&mut self, args: Vec<String>) -> CmdResult {
        if args.len() == 1 {
            return Err(Error::new(ErrorKind::Other, "cd: missing directory"));
        } else if args.len() > 2 {
            let err_msg = format!("cd: too many arguments: {}", args.join(" "));
            return Err(Error::new(ErrorKind::Other, err_msg));
        }

        let dir = &args[1];
        if !std::path::Path::new(&dir).exists() {
            let err_msg = format!("cd: {}: No such file or directory", dir);
            eprintln!("{}", err_msg);
            return Err(Error::new(ErrorKind::Other, err_msg));
        }

        self.current_dir = dir.clone();
        Ok(())
    }

    pub fn run_cmd(mut self) -> CmdResult {
        // check builtin commands
        let args = self.cmd_args[0].get_args().clone();
        let envs = self.cmd_args[0].get_envs().clone();
        let cmd = &args[0].as_str();
        let is_builtin = tls_get!(CMD_MAP).contains_key(cmd);
        if cmd == &"cd" {
            return self.run_cd_cmd(args);
        } else if is_builtin {
            return Self::to_cmd_result(tls_get!(CMD_MAP)[cmd](args, envs));
        }

        self.spawn()?.wait_cmd_result()
    }

    pub fn run_fun(mut self) -> FunResult {
        self.pipes.last_mut().unwrap().stdout(Stdio::piped());

        // check builtin commands
        let args = self.cmd_args[0].get_args().clone();
        let envs = self.cmd_args[0].get_envs().clone();
        let cmd = &args[0].as_str();
        let is_builtin = tls_get!(CMD_MAP).contains_key(cmd);
        if is_builtin {
            return tls_get!(CMD_MAP)[cmd](args, envs);
        }

        self.spawn()?.wait_fun_result()
    }

    fn to_io_error(command: &str, status: ExitStatus) -> Error {
        if let Some(code) = status.code() {
            Error::new(ErrorKind::Other, format!("{} exit with {}", command, code))
        } else {
            Error::new(ErrorKind::Other, "Unknown error")
        }
    }

    fn to_cmd_result(res: FunResult) -> CmdResult {
        match res {
            Ok(v) => {
                print!("{}{}", v, if v.is_empty() { "" } else { "\n" });
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

#[doc(hidden)]
pub enum FdOrFile {
    Fd(i32, bool),          // fd, append?
    File(String, bool),     // file, append?
    OpenedFile(File, bool), // opened file, append?
}

#[doc(hidden)]
#[derive(Default)]
pub struct Cmd {
    args: Vec<String>,
    envs: HashMap<String, String>,
    redirects: Vec<(i32, FdOrFile)>,
}

impl Cmd {
    pub fn add_arg(mut self, arg: String) -> Self {
        if self.is_empty() {
            let v: Vec<&str> = arg.split('=').collect();
            if v.len() == 2 && v[0].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                self.envs.insert(v[0].to_owned(), v[1].to_owned());
                return self;
            }
        }
        self.args.push(arg);
        self
    }

    pub fn add_args(mut self, args: Vec<String>) -> Self {
        for arg in args {
            self = self.add_arg(arg);
        }
        self
    }

    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            args: args.into_iter().map(|s| s.as_ref().to_owned()).collect(),
            envs: HashMap::new(),
            redirects: vec![],
        }
    }

    pub fn get_args(&mut self) -> &mut Vec<String> {
        &mut self.args
    }

    pub fn get_envs(&mut self) -> &mut HashMap<String, String> {
        &mut self.envs
    }

    pub fn set_redirect(mut self, fd: i32, target: FdOrFile) -> Self {
        self.redirects.push((fd, target));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }

    pub fn gen_command(&mut self) -> Command {
        let cmd_args: Vec<String> = self.get_args().to_vec();
        let mut cmd = Command::new(&cmd_args[0]);
        cmd.args(&cmd_args[1..]);

        for (fd_src, target) in self.redirects.iter_mut() {
            match &target {
                FdOrFile::Fd(fd, _append) => {
                    let out = unsafe { Stdio::from_raw_fd(*fd) };
                    match *fd_src {
                        1 => cmd.stdout(out),
                        2 => cmd.stderr(out),
                        _ => panic!("invalid fd: {}", *fd_src),
                    };
                }
                FdOrFile::File(file, append) => {
                    if file == "/dev/null" {
                        match *fd_src {
                            0 => cmd.stdin(Stdio::null()),
                            1 => cmd.stdout(Stdio::null()),
                            2 => cmd.stderr(Stdio::null()),
                            _ => panic!("invalid fd: {}", *fd_src),
                        };
                    } else {
                        let f = if *fd_src == 0 {
                            OpenOptions::new().read(true).open(file).unwrap()
                        } else {
                            OpenOptions::new()
                                .create(true)
                                .truncate(!append)
                                .write(true)
                                .append(*append)
                                .open(file)
                                .unwrap()
                        };
                        let fd = f.as_raw_fd();
                        let out = unsafe { Stdio::from_raw_fd(fd) };
                        match *fd_src {
                            0 => cmd.stdin(out),
                            1 => cmd.stdout(out),
                            2 => cmd.stderr(out),
                            _ => panic!("invalid fd: {}", *fd_src),
                        };
                        *target = FdOrFile::OpenedFile(f, *append);
                    }
                }
                _ => {
                    panic!("file is already opened");
                }
            };
        }

        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_piped_cmds() {
        assert!(Cmds::from_cmd(Cmd::from_args(vec!["echo", "rust"]))
            .pipe(Cmd::from_args(vec!["wc"]))
            .run_cmd()
            .is_ok());
    }

    #[test]
    fn test_run_piped_funs() {
        assert_eq!(
            Cmds::from_cmd(Cmd::from_args(vec!["echo", "rust"]))
                .run_fun()
                .unwrap(),
            "rust"
        );

        assert_eq!(
            Cmds::from_cmd(Cmd::from_args(vec!["echo", "rust"]))
                .pipe(Cmd::from_args(vec!["wc", "-c"]))
                .run_fun()
                .unwrap()
                .trim(),
            "5"
        );
    }

    #[test]
    fn test_stdout_redirect() {
        let tmp_file = "/tmp/file_echo_rust";
        let mut write_cmd = Cmd::from_args(vec!["echo", "rust"]);
        write_cmd = write_cmd.set_redirect(1, FdOrFile::File(tmp_file.to_string(), false));
        assert!(Cmds::from_cmd(write_cmd).run_cmd().is_ok());

        let read_cmd = Cmd::from_args(vec!["cat", tmp_file]);
        assert_eq!(Cmds::from_cmd(read_cmd).run_fun().unwrap(), "rust");

        let cleanup_cmd = Cmd::from_args(vec!["rm", tmp_file]);
        assert!(Cmds::from_cmd(cleanup_cmd).run_cmd().is_ok());
    }
}
