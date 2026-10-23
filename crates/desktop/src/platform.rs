use recomphub_shared::HostPlatform;
#[cfg(windows)]
use recomphub_shared::InstallManifest;
#[cfg(any(target_os = "macos", windows))]
use recomphub_shared::{CpuArch, HostOs};
#[cfg(windows)]
use serde::{Deserialize, Serialize};

use crate::DesktopError;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

/// Detects the actual host rather than treating the compiler target as the running machine.
#[cfg(any(target_os = "macos", windows))]
pub fn detect_host_platform() -> Result<HostPlatform, DesktopError> {
    #[cfg(target_os = "macos")]
    let os = HostOs::Macos;
    #[cfg(windows)]
    let os = HostOs::Windows;

    #[cfg(target_os = "macos")]
    let arch = {
        use std::ffi::CString;
        let name = CString::new("hw.optional.arm64").expect("static name");
        let mut value: i32 = 0;
        let mut size = std::mem::size_of::<i32>();
        // SAFETY: pointers reference initialized storage with its exact size.
        let result = unsafe {
            unsafe extern "C" {
                fn sysctlbyname(
                    name: *const i8,
                    oldp: *mut i32,
                    oldlenp: *mut usize,
                    newp: *mut i32,
                    newlen: usize,
                ) -> i32;
            }
            sysctlbyname(
                name.as_ptr(),
                &mut value,
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if result != 0 || size != std::mem::size_of::<i32>() {
            return Err(DesktopError::UnsupportedHost(
                "could not detect the Mac CPU architecture".into(),
            ));
        }
        if value != 0 {
            CpuArch::Aarch64
        } else {
            CpuArch::X86_64
        }
    };

    #[cfg(windows)]
    let arch = {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
        let mut process_machine = 0u16;
        let mut native_machine = 0u16;
        // SAFETY: process handle is the current process and both output pointers are valid.
        let ok = unsafe {
            IsWow64Process2(
                GetCurrentProcess(),
                &mut process_machine,
                &mut native_machine,
            )
        };
        if ok == 0 {
            return Err(DesktopError::UnsupportedHost(
                "could not detect the Windows CPU architecture".into(),
            ));
        }
        match native_machine {
            0x8664 => CpuArch::X86_64,
            0xAA64 => CpuArch::Aarch64,
            _ => {
                return Err(DesktopError::UnsupportedHost(
                    "this Windows CPU architecture is unsupported".into(),
                ));
            }
        }
    };

    let os_version = sysinfo::System::os_version()
        .ok_or_else(|| DesktopError::UnsupportedHost("OS version is unavailable".into()))?;
    let host = HostPlatform {
        os,
        arch,
        os_version,
    };
    host.validate()
        .map_err(|error| DesktopError::UnsupportedHost(error.to_string()))?;
    Ok(host)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn detect_host_platform() -> Result<HostPlatform, DesktopError> {
    Err(DesktopError::UnsupportedHost(format!(
        "{} is not supported",
        std::env::consts::OS
    )))
}

/// Checks a numeric minimum version using the same shared policy as offer matching.
pub fn meets_minimum_os(host: &HostPlatform, minimum: Option<&str>) -> Result<(), DesktopError> {
    host.validate()
        .map_err(|error| DesktopError::UnsupportedHost(error.to_string()))?;
    if let Some(minimum) = minimum {
        let matches = recomphub_shared::numeric_version_at_least(&host.os_version, minimum)
            .map_err(|error| DesktopError::UnsupportedHost(error.to_string()))?;
        if !matches {
            return Err(DesktopError::UnsupportedHost(format!(
                "requires OS version {minimum} or newer"
            )));
        }
    }
    Ok(())
}

pub(crate) struct TrackedChild {
    #[cfg(not(windows))]
    child: std::process::Child,
    #[cfg(windows)]
    handle: std::os::windows::io::OwnedHandle,
    #[cfg(windows)]
    pid: u32,
    #[cfg(windows)]
    pub(crate) started_at: u64,
}

impl TrackedChild {
    pub(crate) fn id(&self) -> u32 {
        #[cfg(not(windows))]
        {
            self.child.id()
        }
        #[cfg(windows)]
        {
            self.pid
        }
    }

    pub(crate) fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, DesktopError> {
        #[cfg(not(windows))]
        {
            Ok(self.child.try_wait()?)
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            let result = unsafe { WaitForSingleObject(self.handle.as_raw_handle() as _, 0) };
            if result == 0 {
                let code = process_exit_code(self.handle.as_raw_handle() as _)?;
                Ok(Some(exit_status_from_code(code)?))
            } else if result == 0x102 {
                Ok(None)
            } else {
                Err(DesktopError::Package("native process wait failed".into()))
            }
        }
    }
    #[cfg(all(windows, test))]
    pub(crate) fn wait(&mut self) -> Result<std::process::ExitStatus, DesktopError> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle() as _, u32::MAX) } != 0 {
            return Err(DesktopError::Package("native process wait failed".into()));
        }
        exit_status_from_code(process_exit_code(self.handle.as_raw_handle() as _)?)
    }
    pub(crate) fn terminate_and_wait(&mut self) -> Result<(), DesktopError> {
        #[cfg(not(windows))]
        {
            self.child.kill()?;
            let _ = self.child.wait()?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError};
            use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
            let handle = self.handle.as_raw_handle() as _;
            let stopped = unsafe { TerminateProcess(handle, 1) };
            if stopped == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_ACCESS_DENIED {
                    return Err(DesktopError::Package(
                        "could not stop an untracked game process after launch failure".into(),
                    ));
                }
                match unsafe { WaitForSingleObject(handle, 0) } {
                    0 => return Ok(()),
                    0x102 => return Err(DesktopError::Package(
                        "could not terminate the still-running game process after launch failure"
                            .into(),
                    )),
                    _ => {
                        return Err(DesktopError::Package(
                            "could not confirm game process state after launch failure".into(),
                        ));
                    }
                }
            }
            if unsafe { WaitForSingleObject(handle, 5000) } != 0 {
                return Err(DesktopError::Package(
                    "could not confirm game process exit after launch failure".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn launch_process(
    executable: &std::path::Path,
    cwd: &std::path::Path,
    args: &[String],
) -> Result<TrackedChild, DesktopError> {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = crate::macos_app_bundle(executable) {
        assess_macos_bundle(bundle)?;
    }
    let child = std::process::Command::new(executable)
        .current_dir(cwd)
        .args(args)
        .env_remove("APP_FOLDER_PATH")
        .spawn()?;
    Ok(TrackedChild { child })
}

#[cfg(target_os = "macos")]
fn assess_macos_bundle(bundle: &std::path::Path) -> Result<(), DesktopError> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    // Direct exec can leave a quarantined app held before dyld, which is not a running game.
    // Ask the normal system policy first; never alter quarantine, signing or approval settings.
    let mut assessment = Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute", "--"])
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = assessment.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(DesktopError::Package(
                    "macOS did not authorize this app. Review the upstream release; RecompHub has not changed quarantine or security settings.".into(),
                ))
            };
        }
        if Instant::now() >= deadline {
            assessment.kill()?;
            assessment.wait()?;
            return Err(DesktopError::Package(
                "macOS app verification timed out. Try again when system verification is available; no game was started.".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
pub(crate) fn launch_process_verified(
    library_root: &std::path::Path,
    package_directory: &std::path::Path,
    manifest: &InstallManifest,
) -> Result<TrackedChild, DesktopError> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    struct HelperProcess(std::process::Child);
    impl Drop for HelperProcess {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    let absolute_executable = package_directory
        .join(&manifest.recipe.launch.executable)
        .canonicalize()?;
    let absolute_working_directory = package_directory
        .join(&manifest.recipe.launch.working_directory)
        .canonicalize()?;
    let executable = std::env::current_exe()?;
    let request = ShellLaunchRequest {
        parent_pid: std::process::id(),
        parent_started_at: process_start_time_from_handle(unsafe { GetCurrentProcess() })?,
        library_root: library_root.to_string_lossy().into_owned(),
        package_directory: package_directory.to_string_lossy().into_owned(),
        absolute_executable: absolute_executable.to_string_lossy().into_owned(),
        absolute_working_directory: absolute_working_directory.to_string_lossy().into_owned(),
        arguments: manifest.recipe.launch.arguments.clone(),
        manifest: manifest.clone(),
    };
    let bytes = serde_json::to_vec(&request).map_err(|error| {
        DesktopError::Package(format!("could not encode verified launch request: {error}"))
    })?;
    if bytes.len() > SHELL_PROTOCOL_LIMIT {
        return Err(DesktopError::Package(
            "verified launch request exceeds its protocol limit".into(),
        ));
    }
    let mut command = Command::new(executable);
    #[cfg(not(test))]
    command.arg(SHELL_HELPER_ARGUMENT);
    #[cfg(test)]
    command.args([
        "--ignored",
        "--exact",
        "platform::windows_shell_launch_test_helper_entry",
        "--nocapture",
    ]);
    let mut helper = HelperProcess(
        command
            .env_remove("APP_FOLDER_PATH")
            .creation_flags(0x0800_0000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let helper_pid = helper.0.id();
    let mut input = helper
        .0
        .stdin
        .take()
        .ok_or_else(|| DesktopError::Package("launch helper input is unavailable".into()))?;
    write_frame(&mut input, &bytes).map_err(|error| {
        DesktopError::Package(format!(
            "could not deliver verified launch request: {error}"
        ))
    })?;
    drop(input);
    let mut output = helper
        .0
        .stdout
        .take()
        .ok_or_else(|| launch_uncertain("launch helper output is unavailable"))?;
    let response = read_helper_response(&mut output)?;
    let handle = response
        .handle
        .filter(|value| *value != 0 && *value != usize::MAX)
        .map(|value| unsafe { OwnedHandle::from_raw_handle(value as *mut _) });
    if let Some(error) = classify_shell_error(&response) {
        drop(handle);
        return Err(error);
    }
    let handle = handle.ok_or_else(|| {
        launch_uncertain("Windows launch helper did not transfer a process handle")
    })?;
    let (pid, started_at) = match (response.pid, response.started_at) {
        (Some(pid), Some(started_at)) if !response.uncertain => (pid, started_at),
        _ => {
            return Err(launch_uncertain(
                "Windows launch helper returned an invalid process identity",
            ));
        }
    };
    let executable = package_directory
        .join(&manifest.recipe.launch.executable)
        .canonicalize()
        .map_err(|error| {
            launch_uncertain(format!(
                "launched game executable identity is unavailable: {error}"
            ))
        })?;
    let child = TrackedChild {
        handle,
        pid,
        started_at,
    };
    if pid == helper_pid
        || unsafe {
            windows_sys::Win32::System::Threading::GetProcessId(child.handle.as_raw_handle() as _)
        } != pid
        || process_start_time_from_handle(child.handle.as_raw_handle() as _)
            .map_err(|error| launch_uncertain(error.to_string()))?
            != started_at
        || process_image_from_handle(child.handle.as_raw_handle() as _)
            .map_err(|error| launch_uncertain(error.to_string()))?
            != executable
    {
        return Err(launch_uncertain(
            "Windows returned a mismatched process identity",
        ));
    }
    helper
        .0
        .wait()
        .map_err(|error| launch_uncertain(format!("could not reap launch helper: {error}")))?;
    Ok(child)
}

#[cfg(windows)]
fn launch_uncertain(message: impl std::fmt::Display) -> DesktopError {
    DesktopError::LaunchUncertain(message.to_string())
}

#[cfg(windows)]
fn read_helper_response(
    input: &mut impl std::io::Read,
) -> Result<ShellLaunchResponse, DesktopError> {
    read_response_frame(input).map_err(|error| {
        launch_uncertain(format!("could not read launch-helper response: {error}"))
    })
}

#[cfg(windows)]
fn classify_shell_error(response: &ShellLaunchResponse) -> Option<DesktopError> {
    response.error.as_ref().map(|message| {
        let inconsistent =
            response.pid.is_some() || response.started_at.is_some() || response.handle.is_some();
        if response.uncertain || inconsistent {
            launch_uncertain(message)
        } else {
            DesktopError::Package(message.clone())
        }
    })
}

#[cfg(windows)]
const SHELL_HELPER_ARGUMENT: &str = "--recomphub-shell-launch-helper";
#[cfg(windows)]
const SHELL_PROTOCOL_LIMIT: usize = 64 * 1024;
#[cfg(windows)]
const SHELL_PROTOCOL_MAGIC: &[u8; 4] = b"RHSL";

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct ShellLaunchRequest {
    parent_pid: u32,
    parent_started_at: u64,
    library_root: String,
    package_directory: String,
    absolute_executable: String,
    absolute_working_directory: String,
    arguments: Vec<String>,
    manifest: InstallManifest,
}

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct ShellLaunchResponse {
    pid: Option<u32>,
    started_at: Option<u64>,
    handle: Option<usize>,
    error: Option<String>,
    uncertain: bool,
}

#[cfg(windows)]
fn write_frame(output: &mut impl std::io::Write, bytes: &[u8]) -> Result<(), DesktopError> {
    output.write_all(SHELL_PROTOCOL_MAGIC)?;
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(bytes)?;
    output.flush()?;
    Ok(())
}

#[cfg(windows)]
fn read_response_frame<T: for<'de> Deserialize<'de>>(
    input: &mut impl std::io::Read,
) -> Result<T, DesktopError> {
    let mut matched = 0;
    for _ in 0..4096 {
        let mut byte = [0];
        input.read_exact(&mut byte)?;
        matched = if byte[0] == SHELL_PROTOCOL_MAGIC[matched] {
            matched + 1
        } else if byte[0] == SHELL_PROTOCOL_MAGIC[0] {
            1
        } else {
            0
        };
        if matched == SHELL_PROTOCOL_MAGIC.len() {
            break;
        }
    }
    if matched != SHELL_PROTOCOL_MAGIC.len() {
        return Err(DesktopError::Package(
            "launch helper response frame is missing".into(),
        ));
    }
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > SHELL_PROTOCOL_LIMIT {
        return Err(DesktopError::Package(
            "launch helper response exceeds its protocol limit".into(),
        ));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| DesktopError::Package("launch helper response is invalid".into()))
}

#[cfg(windows)]
struct RemoteProcessHandle {
    parent: OwnedHandle,
    handle: windows_sys::Win32::Foundation::HANDLE,
    delivered: bool,
}

#[cfg(windows)]
impl Drop for RemoteProcessHandle {
    fn drop(&mut self) {
        if self.delivered {
            return;
        }
        use windows_sys::Win32::Foundation::{
            DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut local = std::ptr::null_mut();
        let recovered = unsafe {
            DuplicateHandle(
                self.parent.as_raw_handle() as _,
                self.handle,
                GetCurrentProcess(),
                &mut local,
                0,
                0,
                DUPLICATE_CLOSE_SOURCE | DUPLICATE_SAME_ACCESS,
            )
        };
        if recovered != 0 && !local.is_null() {
            drop(unsafe { OwnedHandle::from_raw_handle(local as _) });
        }
    }
}

#[cfg(windows)]
struct PendingShellReply {
    response: ShellLaunchResponse,
    transfer: Option<RemoteProcessHandle>,
}

#[cfg(windows)]
fn publish_shell_reply(
    result: Result<PendingShellReply, DesktopError>,
) -> Result<(), DesktopError> {
    let mut reply = result.unwrap_or_else(|error| PendingShellReply {
        response: ShellLaunchResponse {
            pid: None,
            started_at: None,
            handle: None,
            uncertain: matches!(&error, DesktopError::LaunchUncertain(_)),
            error: Some(error.to_string()),
        },
        transfer: None,
    });
    let launched = reply.response.uncertain || reply.response.error.is_none();
    let delivery_error = |message: String| {
        if launched {
            launch_uncertain(message)
        } else {
            DesktopError::Package(message)
        }
    };
    let bytes = serde_json::to_vec(&reply.response)
        .map_err(|_| delivery_error("could not encode launch helper response".into()))?;
    if bytes.len() > SHELL_PROTOCOL_LIMIT {
        return Err(delivery_error(
            "launch helper response exceeds its protocol limit".into(),
        ));
    }
    write_frame(&mut std::io::stdout().lock(), &bytes).map_err(|error| {
        delivery_error(format!("could not deliver launch helper response: {error}"))
    })?;
    if let Some(transfer) = reply.transfer.as_mut() {
        transfer.delivered = true;
    }
    Ok(())
}

#[cfg(windows)]
fn process_start_time_from_handle(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<u64, DesktopError> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(DesktopError::Package(
            "could not inspect game process creation time".into(),
        ));
    }
    Ok(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
}

#[cfg(windows)]
fn process_image_from_handle(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<std::path::PathBuf, DesktopError> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Threading::QueryFullProcessImageNameW;
    let mut name = [0u16; 32768];
    let mut length = name.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle, 0, name.as_mut_ptr(), &mut length) } == 0 {
        return Err(DesktopError::Package(
            "could not inspect game process executable identity".into(),
        ));
    }
    Ok(
        std::path::PathBuf::from(std::ffi::OsString::from_wide(&name[..length as usize]))
            .canonicalize()?,
    )
}

#[cfg(windows)]
fn shell_execute_manifest(
    package_directory: &std::path::Path,
    manifest: &InstallManifest,
) -> Result<TrackedChild, DesktopError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_CANCELLED, GetLastError};
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    let host = detect_host_platform()?;
    crate::validate_install_directory(package_directory, manifest, &host)?;
    let initialized = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    if initialized < 0 {
        return Err(DesktopError::Package(
            "could not initialize the Windows COM-STA launch helper".into(),
        ));
    }
    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _com = ComGuard;
    let executable = package_directory.join(&manifest.recipe.launch.executable);
    let working = package_directory.join(&manifest.recipe.launch.working_directory);
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let file: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let directory: Vec<u16> = working.as_os_str().encode_wide().chain(Some(0)).collect();
    let parameters_text = manifest
        .recipe
        .launch
        .arguments
        .iter()
        .map(|arg| quote_windows_argument(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let parameters: Vec<u16> = parameters_text.encode_utf16().chain(Some(0)).collect();
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.lpDirectory = directory.as_ptr();
    info.nShow = 1;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = unsafe { GetLastError() };
        return Err(if error == ERROR_CANCELLED {
            DesktopError::Package("Windows launch was cancelled by the user".into())
        } else {
            DesktopError::Package(format!(
                "Windows could not open the verified game executable (error {error})"
            ))
        });
    }
    if info.hProcess.is_null() {
        return Err(launch_uncertain(
            "ShellExecuteEx succeeded without returning a process handle",
        ));
    }
    let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(info.hProcess as _) };
    let pid = unsafe { windows_sys::Win32::System::Threading::GetProcessId(info.hProcess) };
    if pid == 0 {
        return Err(launch_uncertain(
            "game process identity could not be confirmed",
        ));
    }
    let started_at = process_start_time_from_handle(info.hProcess).map_err(|error| {
        launch_uncertain(format!("could not inspect launched game process: {error}"))
    })?;
    Ok(TrackedChild {
        handle,
        pid,
        started_at,
    })
}

#[cfg(windows)]
fn serve_shell_launch_helper() -> Result<PendingShellReply, DesktopError> {
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    let mut input = std::io::stdin().lock();
    let mut header = [0u8; 8];
    input.read_exact(&mut header)?;
    if &header[..4] != SHELL_PROTOCOL_MAGIC {
        return Err(DesktopError::Package(
            "launch helper request frame is invalid".into(),
        ));
    }
    let length =
        u32::from_le_bytes(header[4..8].try_into().expect("four-byte frame length")) as usize;
    if length == 0 || length > SHELL_PROTOCOL_LIMIT {
        return Err(DesktopError::Package(
            "launch helper request exceeds its protocol limit".into(),
        ));
    }
    let mut bytes = vec![0u8; length];
    input.read_exact(&mut bytes)?;
    let mut trailing = [0u8; 1];
    if input.read(&mut trailing)? != 0 {
        return Err(DesktopError::Package(
            "launch helper request contains trailing data".into(),
        ));
    }
    let request: ShellLaunchRequest = serde_json::from_slice(&bytes).map_err(|error| {
        DesktopError::Package(format!("launch helper request is invalid: {error}"))
    })?;
    request
        .manifest
        .validate()
        .map_err(|error| DesktopError::Package(error.to_string()))?;
    let root = validated_absolute_directory(&request.library_root)?;
    let package = validated_absolute_directory(&request.package_directory)?;
    let relative = crate::relative(&root, &package)?;
    crate::owned_version_path(&root, request.manifest.port_id, &relative)?;
    let executable = validated_absolute_file(&request.absolute_executable)?;
    let working = validated_absolute_directory(&request.absolute_working_directory)?;
    let expected_executable = package
        .join(&request.manifest.recipe.launch.executable)
        .canonicalize()?;
    let expected_working = package
        .join(&request.manifest.recipe.launch.working_directory)
        .canonicalize()?;
    if executable != expected_executable
        || working != expected_working
        || request.arguments != request.manifest.recipe.launch.arguments
    {
        return Err(DesktopError::Package("launch helper executable, working directory, or arguments do not match reviewed metadata".into()));
    }
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let parent = unsafe {
        OpenProcess(
            PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            request.parent_pid,
        )
    };
    if parent.is_null() {
        return Err(DesktopError::Package(
            "the desktop process is unavailable for handle transfer".into(),
        ));
    }
    let parent = unsafe { OwnedHandle::from_raw_handle(parent as _) };
    if process_start_time_from_handle(parent.as_raw_handle() as _)? != request.parent_started_at
        || process_image_from_handle(parent.as_raw_handle() as _)?
            != std::env::current_exe()?.canonicalize()?
    {
        return Err(DesktopError::Package(
            "launch helper parent identity does not match the desktop process".into(),
        ));
    }
    let child = shell_execute_manifest(&package, &request.manifest)?;
    let pid = child.id();
    let started_at = child.started_at;
    let process_handle = child.handle.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let mut transferred = std::ptr::null_mut();
    let duplicated = unsafe {
        windows_sys::Win32::Foundation::DuplicateHandle(
            GetCurrentProcess(),
            process_handle,
            parent.as_raw_handle() as _,
            &mut transferred,
            0x0010_1001,
            0,
            0,
        )
    };
    if duplicated == 0 {
        return Err(launch_uncertain(
            "could not transfer game process ownership to the desktop process",
        ));
    }
    Ok(PendingShellReply {
        response: ShellLaunchResponse {
            pid: Some(pid),
            started_at: Some(started_at),
            handle: Some(transferred as usize),
            error: None,
            uncertain: false,
        },
        transfer: Some(RemoteProcessHandle {
            parent,
            handle: transferred,
            delivered: false,
        }),
    })
}

#[cfg(windows)]
fn validated_absolute_directory(value: &str) -> Result<std::path::PathBuf, DesktopError> {
    let path = std::path::Path::new(value);
    if !path.is_absolute() {
        return Err(DesktopError::Package(
            "launch helper paths must be absolute".into(),
        ));
    }
    let canonical = path.canonicalize()?;
    if canonical != path || !canonical.is_dir() {
        return Err(DesktopError::Package(
            "launch helper directory is not canonical".into(),
        ));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if crate::is_link_or_reparse(&metadata) {
        return Err(DesktopError::Package(
            "launch helper directory cannot be a reparse point".into(),
        ));
    }
    Ok(canonical)
}
#[cfg(windows)]
fn validated_absolute_file(value: &str) -> Result<std::path::PathBuf, DesktopError> {
    let path = std::path::Path::new(value);
    if !path.is_absolute() {
        return Err(DesktopError::Package(
            "launch helper executable path must be absolute".into(),
        ));
    }
    let canonical = path.canonicalize()?;
    if canonical != path {
        return Err(DesktopError::Package(
            "launch helper executable path is not canonical".into(),
        ));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || crate::is_link_or_reparse(&metadata) {
        return Err(DesktopError::Package(
            "launch helper executable must be a regular non-reparse file".into(),
        ));
    }
    Ok(canonical)
}

#[cfg(windows)]
pub(crate) fn run_shell_launch_helper_if_requested() -> Result<bool, DesktopError> {
    let mut process_args = std::env::args_os();
    let _ = process_args.next();
    if process_args.next().as_deref() != Some(std::ffi::OsStr::new(SHELL_HELPER_ARGUMENT))
        || process_args.next().is_some()
    {
        return Ok(false);
    }
    publish_shell_reply(serve_shell_launch_helper())?;
    Ok(true)
}

#[cfg(all(windows, test))]
#[test]
#[ignore]
fn windows_shell_launch_test_helper_entry() {
    publish_shell_reply(serve_shell_launch_helper()).expect("publish native helper response");
}
#[cfg(windows)]
fn quote_windows_argument(argument: &str) -> String {
    let mut result = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        if character == '"' {
            result.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
            result.push('"');
        } else {
            result.extend(std::iter::repeat('\\').take(backslashes));
            result.push(character);
        }
        backslashes = 0;
    }
    result.extend(std::iter::repeat('\\').take(backslashes * 2));
    result.push('"');
    result
}

#[cfg(windows)]
fn process_exit_code(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<u32, DesktopError> {
    let mut code = 0;
    if unsafe { windows_sys::Win32::System::Threading::GetExitCodeProcess(handle, &mut code) } == 0
    {
        return Err(DesktopError::Package(
            "could not read game exit status".into(),
        ));
    }
    Ok(code)
}

#[cfg(windows)]
fn exit_status_from_code(code: u32) -> Result<std::process::ExitStatus, DesktopError> {
    use std::os::windows::process::ExitStatusExt;
    Ok(std::process::ExitStatus::from_raw(code))
}

pub(crate) fn process_start_time(
    pid: u32,
    executable: &std::path::Path,
) -> Result<u64, DesktopError> {
    for _ in 0..10 {
        let mut system = sysinfo::System::new_all();
        system.refresh_all();
        if let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) {
            let image = process.exe().ok_or_else(|| {
                DesktopError::Package("launched process executable identity is unavailable".into())
            })?;
            if image.canonicalize()? != executable {
                return Err(DesktopError::Package(
                    "launched process executable identity did not match".into(),
                ));
            }
            #[cfg(any(target_os = "macos", windows))]
            {
                return native_process_start_time(pid);
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            {
                return Ok(process.start_time());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err(DesktopError::Package(
        "launched process could not be identified for durable tracking".into(),
    ))
}

pub(crate) fn process_identity_matches(
    pid: u32,
    started_at: u64,
    executable: &std::path::Path,
) -> Result<bool, DesktopError> {
    let mut system = sysinfo::System::new_all();
    system.refresh_all();
    let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) else {
        return Ok(false);
    };
    let image = process.exe().ok_or_else(|| {
        DesktopError::Package("tracked process executable identity is unavailable".into())
    })?;
    if image.canonicalize()? != executable {
        return Ok(false);
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        Ok(native_process_start_time(pid)? == started_at)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Ok(process.start_time() == started_at)
    }
}

#[cfg(windows)]
fn native_process_start_time(pid: u32) -> Result<u64, DesktopError> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(DesktopError::Package(
            "could not inspect game process creation time".into(),
        ));
    }
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    let result =
        unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    unsafe {
        CloseHandle(handle);
    }
    if result == 0 {
        return Err(DesktopError::Package(
            "could not inspect game process creation time".into(),
        ));
    }
    Ok(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
}

#[cfg(target_os = "macos")]
fn native_process_start_time(pid: u32) -> Result<u64, DesktopError> {
    #[repr(C)]
    struct ProcBsdInfo {
        flags: u32,
        status: u32,
        xstatus: u32,
        pid: u32,
        ppid: u32,
        uid: u32,
        gid: u32,
        ruid: u32,
        rgid: u32,
        svuid: u32,
        svgid: u32,
        rfu: u32,
        comm: [i8; 16],
        name: [i8; 32],
        nfiles: u32,
        pgid: u32,
        pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        nice: i32,
        start_sec: u64,
        start_usec: u64,
    }
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            buffersize: i32,
        ) -> i32;
    }
    let mut info: ProcBsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcBsdInfo>() as i32;
    let read = unsafe {
        proc_pidinfo(
            pid as i32,
            3,
            0,
            (&mut info as *mut ProcBsdInfo).cast(),
            size,
        )
    };
    if read != size {
        return Err(DesktopError::Package(
            "could not inspect game process creation time".into(),
        ));
    }
    info.start_sec
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(info.start_usec))
        .ok_or_else(|| DesktopError::Package("game process creation time overflow".into()))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn native_process_start_time(_pid: u32) -> Result<u64, DesktopError> {
    Err(DesktopError::Package(
        "native high-resolution process timestamps are unavailable on this host".into(),
    ))
}

#[cfg(windows)]
pub(crate) fn mark_download_source(
    path: &std::path::Path,
    source: &str,
) -> Result<(), DesktopError> {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stream)?;
    writeln!(file, "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={source}")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn mark_download_source(
    path: &std::path::Path,
    source: &str,
) -> Result<(), DesktopError> {
    use std::ffi::CString;
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| DesktopError::Package("package path contains NUL".into()))?;
    unsafe extern "C" {
        fn setxattr(
            path: *const i8,
            name: *const i8,
            value: *const std::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> i32;
    }
    let agent = b"com.recomphub.desktop";
    let value = format!(
        "0081;{:x};{};{}",
        chrono::Utc::now().timestamp(),
        std::str::from_utf8(agent).unwrap_or("RecompHub"),
        uuid::Uuid::new_v4()
    );
    let quarantine = CString::new("com.apple.quarantine").expect("static xattr");
    let source_name = CString::new("com.apple.metadata:kMDItemWhereFroms").expect("static xattr");
    // A small XML plist array gives Finder the verified source URL without modifying the app signature.
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><array><string>{}</string></array></plist>",
        xml_escape(source)
    );
    let q_ok = unsafe {
        setxattr(
            path.as_ptr(),
            quarantine.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    let source_ok = unsafe {
        setxattr(
            path.as_ptr(),
            source_name.as_ptr(),
            xml.as_ptr().cast(),
            xml.len(),
            0,
            0,
        )
    };
    if q_ok != 0 || source_ok != 0 {
        return Err(DesktopError::Package(
            "could not preserve macOS quarantine and download-source metadata".into(),
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn mark_download_source(
    _path: &std::path::Path,
    _source: &str,
) -> Result<(), DesktopError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn windows_arguments_round_trip_crt_quoting_rules() {
        assert_eq!(super::quote_windows_argument(""), "\"\"");
        assert_eq!(super::quote_windows_argument("a path"), "\"a path\"");
        assert_eq!(super::quote_windows_argument("a\"b"), "\"a\\\"b\"");
        assert_eq!(super::quote_windows_argument("a path\\"), "\"a path\\\\\"");
    }

    #[cfg(windows)]
    #[test]
    fn helper_response_classifies_uncertainty_without_message_matching() {
        let known = super::ShellLaunchResponse {
            pid: None,
            started_at: None,
            handle: None,
            error: Some("some arbitrary denial text".into()),
            uncertain: false,
        };
        assert!(matches!(
            super::classify_shell_error(&known),
            Some(crate::DesktopError::Package(_))
        ));

        let inconsistent = super::ShellLaunchResponse {
            pid: Some(42),
            ..known
        };
        assert!(matches!(
            super::classify_shell_error(&inconsistent),
            Some(crate::DesktopError::LaunchUncertain(_))
        ));
        let uncertain = super::ShellLaunchResponse {
            uncertain: true,
            ..inconsistent
        };
        assert!(matches!(
            super::classify_shell_error(&uncertain),
            Some(crate::DesktopError::LaunchUncertain(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn malformed_helper_response_is_uncertain() {
        let mut frame = super::SHELL_PROTOCOL_MAGIC.to_vec();
        frame.extend_from_slice(&2u32.to_le_bytes());
        frame.extend_from_slice(b"{}");
        let error = match super::read_helper_response(&mut std::io::Cursor::new(frame)) {
            Err(error) => error,
            Ok(_) => panic!("malformed response was accepted"),
        };
        assert!(matches!(error, crate::DesktopError::LaunchUncertain(_)));
    }
}
