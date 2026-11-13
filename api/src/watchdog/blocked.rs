use std::fs::{self, File};
use std::os::unix::fs::FileExt;

enum Arg {
    None,
    Fd(usize),
    Path(usize),
}

#[cfg(target_arch = "x86_64")]
const SYSCALLS: &[(i64, &str, Arg)] = &[
    (0, "read", Arg::Fd(0)),
    (1, "write", Arg::Fd(0)),
    (2, "open", Arg::Path(0)),
    (3, "close", Arg::Fd(0)),
    (4, "stat", Arg::Path(0)),
    (5, "fstat", Arg::Fd(0)),
    (6, "lstat", Arg::Path(0)),
    (8, "lseek", Arg::Fd(0)),
    (17, "pread64", Arg::Fd(0)),
    (18, "pwrite64", Arg::Fd(0)),
    (19, "readv", Arg::Fd(0)),
    (20, "writev", Arg::Fd(0)),
    (21, "access", Arg::Path(0)),
    (35, "nanosleep", Arg::None),
    (40, "sendfile", Arg::Fd(1)),
    (74, "fsync", Arg::Fd(0)),
    (75, "fdatasync", Arg::Fd(0)),
    (76, "truncate", Arg::Path(0)),
    (77, "ftruncate", Arg::Fd(0)),
    (78, "getdents", Arg::Fd(0)),
    (82, "rename", Arg::Path(0)),
    (83, "mkdir", Arg::Path(0)),
    (84, "rmdir", Arg::Path(0)),
    (87, "unlink", Arg::Path(0)),
    (89, "readlink", Arg::Path(0)),
    (137, "statfs", Arg::Path(0)),
    (138, "fstatfs", Arg::Fd(0)),
    (202, "futex", Arg::None),
    (217, "getdents64", Arg::Fd(0)),
    (230, "clock_nanosleep", Arg::None),
    (257, "openat", Arg::Path(1)),
    (258, "mkdirat", Arg::Path(1)),
    (262, "newfstatat", Arg::Path(1)),
    (263, "unlinkat", Arg::Path(1)),
    (264, "renameat", Arg::Path(1)),
    (267, "readlinkat", Arg::Path(1)),
    (269, "faccessat", Arg::Path(1)),
    (285, "fallocate", Arg::Fd(0)),
    (316, "renameat2", Arg::Path(1)),
    (326, "copy_file_range", Arg::Fd(0)),
    (332, "statx", Arg::Path(1)),
    (439, "faccessat2", Arg::Path(1)),
];

#[cfg(not(target_arch = "x86_64"))]
const SYSCALLS: &[(i64, &str, Arg)] = &[];

/// What an OS thread of this process is doing, read from `/proc`: the syscall it is parked
/// in with the file it touches and the kernel function it waits in, or that it is on CPU.
pub(super) fn describe(thread: i32) -> Option<String> {
    let task = format!("/proc/self/task/{thread}");
    let syscall = fs::read_to_string(format!("{task}/syscall")).ok()?;
    let mut fields = syscall.split_whitespace();
    let nr = fields.next()?;
    if nr == "running" {
        return Some("running on CPU".into());
    }
    let nr: i64 = nr.parse().ok()?;
    let args: Vec<u64> = fields
        .take(6)
        .filter_map(|a| u64::from_str_radix(a.trim_start_matches("0x"), 16).ok())
        .collect();
    // -1: blocked but not in a syscall, which for a worker means a page fault, e.g. on a
    // memory-mapped file whose share stopped answering.
    let call = if nr < 0 {
        "blocked outside a syscall".to_string()
    } else {
        call(nr, &args)
    };
    let wchan = fs::read_to_string(format!("{task}/wchan"))
        .ok()
        .filter(|w| !w.is_empty() && w != "0");
    Some(match wchan {
        Some(w) => format!("{call} in {w}"),
        None => call,
    })
}

fn call(nr: i64, args: &[u64]) -> String {
    let Some((_, name, arg)) = SYSCALLS.iter().find(|(n, ..)| *n == nr) else {
        return format!("syscall {nr}");
    };
    match *arg {
        Arg::None => name.to_string(),
        Arg::Fd(i) => {
            let Some(&fd) = args.get(i) else {
                return name.to_string();
            };
            match fs::read_link(format!("/proc/self/fd/{fd}")) {
                Ok(target) => format!("{name}({})", target.display()),
                Err(_) => format!("{name}(fd {fd})"),
            }
        }
        Arg::Path(i) => match args.get(i).and_then(|&addr| c_string_at(addr)) {
            Some(path) => format!("{name}({path:?})"),
            None => name.to_string(),
        },
    }
}

/// Reads through `/proc/self/mem` rather than the pointer, so an address that is no longer
/// mapped fails the read instead of faulting.
fn c_string_at(addr: u64) -> Option<String> {
    let mem = File::open("/proc/self/mem").ok()?;
    let mut buf = vec![0u8; 4096];
    let n = mem.read_at(&mut buf, addr).ok()?;
    let bytes = &buf[..n];
    let end = bytes.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}
