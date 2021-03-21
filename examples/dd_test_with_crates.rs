// get disk read bandwidth with multiple threads
//
// Usage: dd_test_with_crates [-b <block_size>] [-t <thread_num>] -f <file>
//
// e.g:
// ➜  rust_cmd_lib git:(master) ✗ cargo run --example dd_test_with_crates -- -b 4096 -f /dev/nvme0n1 -t 4
//     Finished dev [unoptimized + debuginfo] target(s) in 1.56s
//      Running `target/debug/examples/dd_test_with_crates -b 4096 -f /dev/nvme0n1 -t 4`
// Dropping caches at first
// Running with thread_num: 4, block_size: 4096
// thread 1 bandwidth: 286MB/s
// thread 3 bandwidth: 269MB/s
// thread 2 bandwidth: 267MB/s
// thread 0 bandwidth: 265MB/s
// Total bandwidth: 1.01 GiB/s
use byte_unit::Byte;
use cmd_lib::*;
use rayon::prelude::*;
use structopt::StructOpt;

const DATA_SIZE: i64 = 10 * 1024 * 1024 * 1024; // 10GB data

#[derive(StructOpt)]
#[structopt(name = "dd_test_with_crates", about = "Get disk read bandwidth.")]
struct Opt {
    #[structopt(short, default_value = "4096")]
    block_size: i32,
    #[structopt(short, default_value = "1")]
    thread_num: i32,
    #[structopt(short)]
    file: String,
}

fn main() -> CmdResult {
    use_builtin_cmd!(echo, info);
    let Opt {
        block_size,
        thread_num,
        file,
    } = Opt::from_args();

    run_cmd! (
        info "Dropping caches at first";
        sudo bash -c "echo 3 > /proc/sys/vm/drop_caches";
        info "Running with thread_num: $thread_num, block_size: $block_size";
    )?;
    let cnt: i32 = (DATA_SIZE / thread_num as i64 / block_size as i64) as i32;
    let total_bandwidth: u128 = (0..thread_num)
        .into_par_iter()
        .map(|i| {
            let off = cnt * i;
            let bandwidth = run_fun!(
                sudo bash -c "dd if=$file of=/dev/null bs=$block_size skip=$off count=$cnt 2>&1"
                | awk r"{print $10 $11}" | cut -d / -f1
            )
            .unwrap();
            let bandwidth = bandwidth.trim();
            run_cmd!(info "thread $i bandwidth: ${bandwidth}/s").unwrap();
            Byte::from_str(bandwidth).unwrap().get_bytes()
        })
        .sum();
    let total_bandwidth = Byte::from_bytes(total_bandwidth).get_appropriate_unit(true);
    run_cmd!(info "Total bandwidth: ${total_bandwidth}/s")?;

    Ok(())
}
