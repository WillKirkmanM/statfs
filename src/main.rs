use clap::{Parser, ValueEnum};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

#[cfg(all(
    target_os = "linux",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
compile_error!(
    "statfs supports Linux only on x86_64 and aarch64; this target has no defined statfs ABI"
);

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Parser)]
#[command(
    version,
    about = "Inspect Linux filesystem statistics for selected paths",
    long_about = "Inspect Linux filesystem statistics for selected paths. Paths are printed as escaped bytes: backslashes, quotes and control characters are escaped, and non-ASCII bytes use \\xNN. JSON path strings contain this same byte notation. Byte capacities use the fragment size, falling back to block size when it is zero; null denotes arithmetic overflow. Free space includes reserved blocks; available space is for unprivileged users."
)]
struct Args {
    #[arg(long, value_enum, default_value = "text", help = "Output format")]
    format: Format,
    #[arg(
        default_value = ".",
        value_name = "PATH",
        help = "Paths to query in argument order"
    )]
    paths: Vec<PathBuf>,
}

fn escape_path(bytes: &[u8]) -> String {
    let mut result = String::new();
    for &byte in bytes {
        for character in std::ascii::escape_default(byte) {
            result.push(char::from(character));
        }
    }
    result
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c <= '\u{1f}' => {
                use std::fmt::Write;
                write!(result, "\\u{:04x}", u32::from(c)).unwrap();
            }
            c => result.push(c),
        }
    }
    result.push('"');
    result
}

fn filesystem_name(kind: u64) -> Option<&'static str> {
    match kind {
        0x0000_ef53 => Some("ext2/ext3/ext4"),
        0x5846_5342 => Some("xfs"),
        0x9123_683e => Some("btrfs"),
        0x0102_1994 => Some("tmpfs"),
        0x8584_58f6 => Some("ramfs"),
        0x794c_7630 => Some("overlayfs"),
        0x0000_6969 => Some("nfs"),
        0xff53_4d42 => Some("cifs"),
        0xfe53_4d42 => Some("smb2"),
        0x6573_5546 => Some("fuse"),
        0x0000_9fa0 => Some("proc"),
        0x6265_6572 => Some("sysfs"),
        0x6367_7270 => Some("cgroup2"),
        0x0027_e0eb => Some("cgroup"),
        0x0000_4d44 => Some("fat"),
        0x2011_bab0 => Some("exfat"),
        0x5346_544e => Some("ntfs"),
        0x7371_7368 => Some("squashfs"),
        0xe0f5_e1e2 => Some("erofs"),
        0xf2f5_2010 => Some("f2fs"),
        0x0000_9660 => Some("iso9660"),
        0x2fc1_2fc1 => Some("zfs"),
        0x0102_1997 => Some("9p"),
        _ => None,
    }
}

struct Statistics {
    filesystem_type: u64,
    filesystem_id: [u32; 2],
    block_size: u64,
    fragment_size: u64,
    maximum_filename_length: u64,
    mount_flags: u64,
    total_blocks: u64,
    free_blocks: u64,
    available_blocks: u64,
    total_inodes: u64,
    free_inodes: u64,
}

impl Statistics {
    fn capacities(&self) -> [Option<u64>; 3] {
        let size = if self.fragment_size == 0 {
            self.block_size
        } else {
            self.fragment_size
        };
        [self.total_blocks, self.free_blocks, self.available_blocks]
            .map(|blocks| blocks.checked_mul(size))
    }

    fn render(&self, path: &str, format: Format) -> String {
        let kind_hex = format!("0x{:x}", self.filesystem_type);
        let kind = filesystem_name(self.filesystem_type).unwrap_or(&kind_hex);
        let [total_bytes, free_bytes, available_bytes] = self.capacities();
        let capacity = |value: Option<u64>| {
            value.map_or_else(
                || match format {
                    Format::Text => "overflow".to_owned(),
                    Format::Json => "null".to_owned(),
                },
                |number| number.to_string(),
            )
        };
        let fields = [
            ("block_size", self.block_size),
            ("fragment_size", self.fragment_size),
            ("maximum_filename_length", self.maximum_filename_length),
            ("mount_flags", self.mount_flags),
            ("total_blocks", self.total_blocks),
            ("free_blocks", self.free_blocks),
            ("available_blocks", self.available_blocks),
            ("total_inodes", self.total_inodes),
            ("free_inodes", self.free_inodes),
        ];
        let capacities = [
            ("total_bytes", total_bytes),
            ("free_bytes", free_bytes),
            ("available_bytes", available_bytes),
        ];
        match format {
            Format::Json => {
                let mut result = format!(
                    "{{\"path\":{},\"filesystem_type\":{},\"filesystem_type_hex\":{},\"filesystem_id\":[{},{}]",
                    json_string(path),
                    json_string(kind),
                    json_string(&kind_hex),
                    self.filesystem_id[0],
                    self.filesystem_id[1]
                );
                for (name, value) in fields {
                    result.push_str(&format!(",\"{name}\":{value}"));
                }
                for (name, value) in capacities {
                    result.push_str(&format!(",\"{name}\":{}", capacity(value)));
                }
                result.push('}');
                result
            }
            Format::Text => {
                let mut result = format!(
                    "path=\"{path}\" filesystem_type={kind} filesystem_type_hex={kind_hex} filesystem_id={:08x}:{:08x}",
                    self.filesystem_id[0], self.filesystem_id[1]
                );
                for (name, value) in fields {
                    if name == "mount_flags" {
                        result.push_str(&format!(" {name}=0x{value:x}"));
                    } else {
                        result.push_str(&format!(" {name}={value}"));
                    }
                }
                for (name, value) in capacities {
                    result.push_str(&format!(" {name}={}", capacity(value)));
                }
                result
            }
        }
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod linux {
    use super::*;
    use std::ffi::{CString, c_char, c_int, c_long, c_ulong};
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[repr(C)]
    struct Statfs {
        filesystem_type: c_long,
        block_size: c_long,
        total_blocks: c_ulong,
        free_blocks: c_ulong,
        available_blocks: c_ulong,
        total_inodes: c_ulong,
        free_inodes: c_ulong,
        filesystem_id: [c_int; 2],
        maximum_filename_length: c_long,
        fragment_size: c_long,
        mount_flags: c_long,
        spare: [c_long; 4],
    }

    const _: () = assert!(size_of::<Statfs>() == 120 && align_of::<Statfs>() == 8);

    unsafe extern "C" {
        fn statfs(path: *const c_char, buffer: *mut Statfs) -> c_int;
    }

    pub fn query(path: &Path) -> io::Result<Statistics> {
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
        let mut buffer = MaybeUninit::<Statfs>::uninit();
        if unsafe { statfs(path.as_ptr(), buffer.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let raw = unsafe { buffer.assume_init() };
        let nonnegative = |value: c_long| {
            u64::try_from(value).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "kernel returned a negative size",
                )
            })
        };
        Ok(Statistics {
            filesystem_type: raw.filesystem_type as u64,
            filesystem_id: raw.filesystem_id.map(|word| word as u32),
            block_size: nonnegative(raw.block_size)?,
            fragment_size: nonnegative(raw.fragment_size)?,
            maximum_filename_length: nonnegative(raw.maximum_filename_length)?,
            mount_flags: raw.mount_flags as u64,
            total_blocks: raw.total_blocks,
            free_blocks: raw.free_blocks,
            available_blocks: raw.available_blocks,
            total_inodes: raw.total_inodes,
            free_inodes: raw.free_inodes,
        })
    }
}

fn run(args: Args) -> io::Result<bool> {
    let mut results = Vec::new();
    let mut failed = false;
    for path in args.paths {
        #[cfg(target_os = "linux")]
        let result = linux::query(&path);
        #[cfg(not(target_os = "linux"))]
        let result: io::Result<Statistics> = Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported platform: statfs requires Linux x86_64 or aarch64",
        ));
        let path = escape_path(path.as_os_str().as_encoded_bytes());
        match result {
            Ok(statistics) => results.push(statistics.render(&path, args.format)),
            Err(error) => {
                failed = true;
                eprintln!("statfs: \"{path}\": {error}");
                if matches!(args.format, Format::Json) {
                    results.push(format!(
                        "{{\"path\":{},\"error\":{}}}",
                        json_string(&path),
                        json_string(&error.to_string())
                    ));
                }
            }
        }
    }
    let output = match args.format {
        Format::Json => format!("[{}]\n", results.join(",")),
        Format::Text if results.is_empty() => String::new(),
        Format::Text => format!("{}\n", results.join("\n")),
    };
    io::stdout().lock().write_all(output.as_bytes())?;
    Ok(!failed)
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("statfs: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statistics() -> Statistics {
        Statistics {
            filesystem_type: 0xdeadbeef,
            filesystem_id: [1, u32::MAX],
            block_size: 4096,
            fragment_size: 512,
            maximum_filename_length: 255,
            mount_flags: 0,
            total_blocks: u64::MAX,
            free_blocks: 8,
            available_blocks: 3,
            total_inodes: 100,
            free_inodes: 90,
        }
    }

    #[test]
    fn capacities_check_overflow_and_preserve_reserved_space() {
        let mut stats = statistics();
        assert_eq!(stats.capacities(), [None, Some(4096), Some(1536)]);
        stats.fragment_size = 0;
        assert_eq!(stats.capacities(), [None, Some(32768), Some(12288)]);
    }

    #[test]
    fn paths_escape_arbitrary_bytes_and_json_metacharacters() {
        let escaped = escape_path(b"a\"\\\n\x00\xff");
        assert_eq!(escaped, "a\\\"\\\\\\n\\x00\\xff");
        assert_eq!(json_string("\"\\\n\u{0}"), "\"\\\"\\\\\\n\\u0000\"");
        assert_eq!(escape_path(b"\\xff"), "\\\\xff");
    }

    #[test]
    fn json_keeps_unknown_type_and_overflow_explicit() {
        let rendered = statistics().render(".", Format::Json);
        assert!(rendered.contains("\"filesystem_type\":\"0xdeadbeef\""));
        assert!(rendered.contains("\"total_bytes\":null"));
        assert!(rendered.contains("\"available_bytes\":1536"));
        assert_eq!(filesystem_name(0xef53), Some("ext2/ext3/ext4"));
    }

    #[test]
    fn parser_defaults_and_rejects_invalid_format() {
        let args = Args::try_parse_from(["statfs"]).unwrap();
        assert_eq!(args.paths, [PathBuf::from(".")]);
        assert!(Args::try_parse_from(["statfs", "--format", "xml", "."]).is_err());
        let args = Args::try_parse_from(["statfs", "second", "first"]).unwrap();
        assert_eq!(
            args.paths,
            [PathBuf::from("second"), PathBuf::from("first")]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn syscall_queries_current_directory_and_rejects_nul() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::path::Path;
        assert!(linux::query(Path::new(".")).unwrap().block_size > 0);
        let error = linux::query(Path::new(OsStr::from_bytes(b"bad\0path")))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
