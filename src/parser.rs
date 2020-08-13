use std::collections::{VecDeque, HashMap};
use crate::process::{GroupCmds, PipedCmds, Cmd};

#[doc(hidden)]
#[macro_export]
macro_rules! parse_string_literal {
    (&$sl:expr;) => {
        $sl
    };
    (&$sl:expr; - $($other:tt)*) => {
        $crate::parse_string_literal!{&$sl; $($other)*}
    };
    (&$sl:expr; $cur:literal $($other:tt)*) => {
        let s = stringify!($cur);
        // only save string literals
        if s.starts_with("\"") || s.starts_with("r") || s.starts_with("b") {
            $sl.push_back($cur.to_string());
        }
        $crate::parse_string_literal!{&$sl; $($other)*}
    };
    (&$sl:expr; $cur:tt $($other:tt)*) => {
        $crate::parse_string_literal!{&$sl; $($other)*}
    };
    ($cur:tt $($other:tt)*) => {{
        let mut __str_lits = std::collections::VecDeque::<String>::new();
        $crate::parse_string_literal!{&__str_lits; $cur $($other)*}
    }};
}

#[doc(hidden)]
pub struct Parser {
    str_lits: VecDeque<String>,
    sym_table: HashMap<String, String>,

    file: &'static str,
    line: u32,

    src: String,
}

impl Parser {
    pub fn new(src: String) -> Self {
        Self {
            str_lits: VecDeque::new(),
            sym_table: HashMap::new(),
            file: "",
            line: 0,
            src,
        }
    }

    pub fn with_lits(&mut self, str_lits: VecDeque<String>) -> &mut Self {
        self.str_lits = str_lits;
        self
    }

    pub fn with_sym_table(&mut self, sym_table: HashMap<String, String>) -> &mut Self {
        self.sym_table = sym_table;
        self
    }

    pub fn with_location(&mut self, file: &'static str, line: u32) -> &mut Self {
        self.file = file;
        self.line = line;
        self
    }

    pub fn parse(&mut self) -> GroupCmds {
        let mut ret = GroupCmds::new();
        let s: Vec<char> = self.src.chars().collect();
        let len = s.len();
        let mut i = 0;

        // skip leading spaces
        while i < len  && char::is_whitespace(s[i]) { i += 1; }
        if i == len { return ret; }

        // skip variables declaration part
        if i < len && s[i] == '|' {
            i += 1;
            while i < len && s[i] != '|' { i += 1; }
            i += 1;
        }

        // real commands parsing starts
        while i < len {
            while i < len && char::is_whitespace(s[i]) { i += 1; }
            if i == len { break; }

            let cmd = self.parse_cmd(&s, &mut i);
            if !cmd.0.is_empty() {
                ret.add(cmd.0, cmd.1);
            }
        }
        ret
    }

    fn parse_cmd(&mut self, s: &Vec<char>, i: &mut usize) -> (PipedCmds, Option<PipedCmds>) {
        let mut ret = vec![PipedCmds::new(), PipedCmds::new()];
        let len = s.len();
        for j in 0..2 {
            while *i < len && s[*i] != ';' {
                while *i < len && char::is_whitespace(s[*i]) { *i += 1; }
                if *i == len { break; }

                let cmd = self.parse_pipe(s, i);
                let pipe_argv = cmd.get_args();
                if !pipe_argv.is_empty() {
                    ret[j].pipe(pipe_argv);
                }
                if *i < len && s[*i] == '|' {
                    break;
                }
            }
            if *i < len && s[*i] == '|' {
                assert_eq!(s[*i + 1], '|');
                *i += 2;    // skip "||" operator
            } else {
                break;
            }
        }
        if *i < len && s[*i] == ';' { *i += 1; }
        let (ret1, ret0) = (ret.pop().unwrap(), ret.pop().unwrap());
        (ret0, if ret1.is_empty() { None } else { Some(ret1) })
    }

    fn parse_pipe(&mut self, s: &Vec<char>, i: &mut usize) -> Cmd {
        let mut ret = Cmd::new();
        let len = s.len();
        while *i < len && s[*i] != '|' && s[*i] != ';' {
            while *i < len && char::is_whitespace(s[*i]) { *i += 1; }
            if *i == len { break; }
            let mut arg = String::new();
            let mut is_ended = false;
            let mut is_str_lit = false;
            let mut is_raw = false;

            while *i < len && !is_ended {
                let mut cnt = 0;    // '#' counts for raw string literal
                if s[*i] == 'r' || s[*i] == 'b' {
                    let mut j = *i + 1;
                    while j < len && s[j] == '#' { j += 1; }
                    if j < len && s[j] == '\"' {
                        is_str_lit = true;
                        is_raw = true;
                        cnt = j - *i - 1;
                        *i = j + 1;
                    }
                } else if s[*i] == '\"' && (*i == 0 || s[*i - 1] != '\\') {
                    is_str_lit = true;
                    *i += 1;
                }

                if is_str_lit {
                    let mut found_end = false;
                    *i += 1;
                    while *i < len && !found_end {
                        if s[*i] == '\"' {
                            let mut cnt2 = cnt;
                            let mut j = *i + 1;
                            while j < len && cnt2 > 0 && s[j] == '#' {
                                cnt2 -= 1;
                                j += 1;
                            }
                            if cnt2 == 0 {
                                found_end = true;
                                *i = j;
                                break;
                            }
                        }
                        *i += 1;
                    }
                    if !found_end {
                        panic!("invalid raw string literal at {}:{}", self.file, self.line);
                    }

                    let str_lit = self.str_lits.pop_front().unwrap();
                    if is_raw {
                        arg += &str_lit; // don't resolve names for raw string literals
                    } else {
                        arg += &crate::sym_table::resolve_name(&str_lit,
                                                               &self.sym_table,
                                                               &self.file,
                                                               self.line);

                    }
                }

                let mut arg1 = String::new();
                //let mut stdout_redirect = String::new();
                while *i < len {
                    if s[*i] == '|' || s[*i] == ';' || char::is_whitespace(s[*i]) {
                        is_ended = true;
                        break;
                    }
                    if s[*i] == '\"' && s[*i - 1] != '\\' {
                        break;
                    }
                    arg1.push(s[*i]);
                    *i += 1;
                }
                if !arg1.is_empty() {
                    arg += &crate::sym_table::resolve_name(&arg1,
                                                           &self.sym_table,
                                                           &self.file,
                                                           self.line);
                }
            }
            if !arg.is_empty() {
                ret.add_arg(arg);
            }
        }
        if *i < len && s[*i] == '|' {
            if *i + 1 < len && s[*i + 1] != '|' {
                *i += 1;
            }
        }
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_string_literal() {
        let str_lits1 = parse_string_literal!(ls "/tmp" "/");
        assert_eq!(str_lits1, ["/tmp", "/"]);

        let str_lits2 = parse_string_literal!(ping -c 3 r"127.0.0.1");
        assert_eq!(str_lits2, ["127.0.0.1"]);

        let str_lits3 = parse_string_literal!(echo r#"rust"cmd_lib"#);
        assert_eq!(str_lits3, ["rust\"cmd_lib"]);
    }

    #[test]
    fn test_parser_or_cmd() {
        assert!(Parser::new("ls /nofile || true; echo continue".to_string())
                .parse()
                .run_cmd()
                .is_ok());
    }
}

