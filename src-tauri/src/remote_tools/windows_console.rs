//! Launch PowerShell with console handles supplied by Windows, not the GUI host.
use super::*;
use base64::Engine;
use std::{
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    process::ExitStatus,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{
        CREATE_NEW_CONSOLE, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION, STARTUPINFOW,
        WaitForSingleObject,
    },
};

pub(super) struct ConsoleProcess(OwnedHandle);

pub(super) fn spawn(script: &str) -> Result<ConsoleProcess> {
    let powershell = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .context("SystemRoot is unavailable")?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let application: Vec<u16> = powershell
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let script: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(script);
    let mut command: Vec<u16> =
        format!("powershell.exe -NoLogo -NoProfile -NoExit -EncodedCommand {encoded}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
    if command.len() > 32767 {
        anyhow::bail!("PowerShell launch command exceeds the Windows command-line limit");
    }

    // SAFETY: These Win32 POD structures accept zero/null in their optional
    // fields. In particular, leave STARTF_USESTDHANDLES unset so Windows creates
    // real read/write console handles instead of inheriting NUL or pipe handles.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: Both strings are NUL-terminated and command is writable. All
    // pointers remain valid for this call. No parent handles are inherited;
    // null environment/current-directory pointers retain the normal defaults.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_NEW_CONSOLE,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error()).context(tr!(
            "Could not launch PowerShell with an interactive console.",
            "대화형 콘솔에서 PowerShell을 실행하지 못했습니다."
        ));
    }
    // SAFETY: A successful CreateProcessW returns two owned, valid handles.
    // Closing them releases our references, not the user's running terminal.
    let handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
    drop(unsafe { OwnedHandle::from_raw_handle(process.hThread) });
    Ok(ConsoleProcess(handle))
}

impl ConsoleProcess {
    pub(super) async fn wait(self) -> io::Result<ExitStatus> {
        loop {
            // SAFETY: The owned process handle remains open for this future.
            match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    // SAFETY: code is writable and the process handle is valid.
                    if unsafe { GetExitCodeProcess(self.0.as_raw_handle(), &mut code) } == 0 {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(ExitStatus::from_raw(code));
                }
                WAIT_TIMEOUT => tokio::time::sleep(Duration::from_millis(500)).await,
                _ => return Err(io::Error::last_os_error()),
            }
        }
    }
}
