#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod drag;

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    capture::run()
}

#[cfg(not(windows))]
fn main() {
    panic!("This installer check requires a Windows desktop");
}
