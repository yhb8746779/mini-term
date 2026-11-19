/// GUI launches on macOS commonly inherit a soft NOFILE limit of only 256.
/// Change this process only; preserve the hard limit and any higher soft limit.
pub fn raise_file_limit() {
    #[cfg(unix)]
    unsafe {
        let mut limit = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            eprintln!("[resources] getrlimit: {}", std::io::Error::last_os_error());
            return;
        }
        let target = limit.rlim_max.min(4096);
        if limit.rlim_cur < target {
            limit.rlim_cur = target;
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                eprintln!("[resources] setrlimit: {}", std::io::Error::last_os_error());
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn raised_limit_supports_many_ptys() {
        // Separate process: lowering a limit must not interfere with parallel tests.
        let result = std::process::Command::new("/bin/sh")
            .arg("-c").arg("ulimit -Sn 256; exec \"$1\" --exact resource_limits::tests::limit_child --nocapture")
            .arg("limit-test").arg(std::env::current_exe().unwrap())
            .env("MINITERM_LIMIT_TEST", "1").output().unwrap();
        assert!(result.status.success(), "{} {}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    }

    #[test]
    fn limit_child() {
        if std::env::var_os("MINITERM_LIMIT_TEST").is_none() { return; }
        super::raise_file_limit();
        let mut pairs = Vec::new();
        for _ in 0..100 {
            let pair = portable_pty::native_pty_system().openpty(portable_pty::PtySize::default()).unwrap();
            let writer = pair.master.take_writer().unwrap();
            let reader = pair.master.try_clone_reader().unwrap();
            pairs.push((pair, writer, reader));
        }
        // More than 400 descriptors open at once, exceeding the previous limit.
        assert_eq!(pairs.len(), 100);
    }
}
