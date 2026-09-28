mod seatbelt;
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use seatbelt::SeatbeltSandbox;
#[cfg(windows)]
pub(crate) use windows::WindowsSandbox;
