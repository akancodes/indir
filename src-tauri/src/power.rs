//! The computer's power: kept from falling asleep while something downloads,
//! and put to sleep or shut down once every download is done, when asked.

use std::sync::mpsc;
use std::thread;

#[cfg(not(target_os = "linux"))]
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Sleep,
    ShutDown,
}

enum Ask {
    Awake(bool),
    Act(Action, mpsc::Sender<Result<(), String>>),
}

/// Talks to the system on a thread of its own, one request at a time and in
/// order, since the system may take a while to answer.
pub struct Power {
    tell: mpsc::Sender<Ask>,
}

impl Power {
    pub fn start() -> Self {
        let (tell, asked) = mpsc::channel();
        let started = thread::Builder::new().name("indir-power".into()).spawn(move || {
            let mut hold = None;
            for ask in asked {
                match ask {
                    Ask::Awake(true) if hold.is_none() => match Hold::new() {
                        Ok(new) => hold = Some(new),
                        Err(e) => eprintln!("indir: couldn't keep the computer awake: {e}"),
                    },
                    Ask::Awake(true) => {}
                    Ask::Awake(false) => hold = None,
                    Ask::Act(action, answer) => {
                        // Nothing may keep the computer awake by now, indir least of all.
                        hold = None;
                        let _ = answer.send(act(action));
                    }
                }
            }
        });
        if let Err(e) = started {
            eprintln!("indir: couldn't start the power thread: {e}");
        }
        Self { tell }
    }

    /// Keeps the computer from falling asleep by itself while `on`. The screen
    /// may still turn off, and closing a laptop's lid still puts it to sleep.
    pub fn keep_awake(&self, on: bool) {
        let _ = self.tell.send(Ask::Awake(on));
    }

    /// Puts the computer to sleep or shuts it down, and says why not if the
    /// system refused. Waits for the system's answer, so never call it on the
    /// main thread.
    pub fn act(&self, action: Action) -> Result<(), String> {
        let (answer, answered) = mpsc::channel();
        let gone = || "indir's power thread has stopped".to_string();
        self.tell.send(Ask::Act(action, answer)).map_err(|_| gone())?;
        answered.recv().map_err(|_| gone())?
    }
}

/// Whether indir may shut the Mac down. macOS asks the person the first
/// time: better while they choose it than once the downloads are done and
/// nobody is there to answer. Waits for the answer.
#[cfg(target_os = "macos")]
pub fn may_shut_down() -> Result<(), String> {
    // Something only System Events can answer: AppleScript knows an app's own
    // name without asking it, and would never send the event at all.
    run(Command::new("/usr/bin/osascript")
        .args(["-e", "tell application \"System Events\" to get name of current user"]))
    .map_err(|e| macos_refusal(&e))
}

/// Other systems don't ask.
#[cfg(not(target_os = "macos"))]
pub fn may_shut_down() -> Result<(), String> {
    Ok(())
}

/// Runs a command, and turns a failure into words.
#[cfg(not(target_os = "linux"))]
fn run(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if said.is_empty() { output.status.to_string() } else { said })
}

#[cfg(target_os = "macos")]
fn macos_refusal(said: &str) -> String {
    // errAEEventNotPermitted: the person said no, now or before.
    if said.contains("-1743") {
        "macOS doesn't let indir shut the Mac down. Allow indir to control System Events in System Settings → \
         Privacy & Security → Automation, then choose Shut down again."
            .into()
    } else {
        said.into()
    }
}

#[cfg(target_os = "macos")]
fn act(action: Action) -> Result<(), String> {
    match action {
        Action::Sleep => run(Command::new("/usr/bin/pmset").arg("sleepnow")),
        // Apps get to save their work first, as with the Apple menu's Shut Down.
        Action::ShutDown => {
            run(Command::new("/usr/bin/osascript").args(["-e", "tell application \"System Events\" to shut down"]))
                .map_err(|e| macos_refusal(&e))
        }
    }
}

#[cfg(target_os = "macos")]
struct Hold(u32);

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        kind: core_foundation::string::CFStringRef,
        level: u32,
        name: core_foundation::string::CFStringRef,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

#[cfg(target_os = "macos")]
impl Hold {
    fn new() -> Result<Self, String> {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;

        // kIOPMAssertionTypePreventUserIdleSystemSleep, at kIOPMAssertionLevelOn.
        let kind = CFString::from_static_string("PreventUserIdleSystemSleep");
        let name = CFString::from_static_string("Downloading files");
        let mut id = 0;
        // SAFETY: both strings live for the whole call, and `id` is a valid place for the answer.
        let result = unsafe {
            IOPMAssertionCreateWithName(kind.as_concrete_TypeRef(), 255, name.as_concrete_TypeRef(), &mut id)
        };
        if result == 0 {
            Ok(Self(id))
        } else {
            Err(format!("IOKit answered {result:#x}"))
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Hold {
    fn drop(&mut self) {
        // SAFETY: the assertion was created by this Hold and is released once.
        unsafe { IOPMAssertionRelease(self.0) };
    }
}

#[cfg(windows)]
fn act(action: Action) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Wdk::System::SystemServices::NtPowerInformation;
    use windows_sys::Win32::System::Power::{
        GetPwrCapabilities, ScreenOff, SetSuspendState, SYSTEM_POWER_CAPABILITIES,
    };
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    match action {
        Action::Sleep => {
            // SAFETY: zeroed is a valid SYSTEM_POWER_CAPABILITIES, which the call fills in.
            let mut capabilities: SYSTEM_POWER_CAPABILITIES = unsafe { std::mem::zeroed() };
            // SAFETY: the pointer is to a value of the right type, alive for the call.
            let modern_standby = unsafe { GetPwrCapabilities(&mut capabilities) } && capabilities.AoAc;
            // Machines with modern standby have no sleep of the old kind, and
            // asking for it hibernates them. Turning the screen off starts their
            // standby, as the Start menu's Sleep does (Open-Shell does the same).
            // SAFETY: ScreenOff takes no buffers.
            if modern_standby
                && unsafe { NtPowerInformation(ScreenOff, std::ptr::null(), 0, std::ptr::null_mut(), 0) } >= 0
            {
                return Ok(());
            }
            // Hibernating is still better than staying on, if standby refused.
            // SAFETY: no pointers; the call returns once the computer wakes up again.
            if unsafe { SetSuspendState(false, false, false) } {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().to_string())
            }
        }
        Action::ShutDown => {
            // By its full path, never a shutdown.exe that happens to sit next to indir.
            let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let shutdown = std::path::Path::new(&system).join("System32").join("shutdown.exe");
            // Without /f: an app with unsaved work may still ask to keep it.
            run(Command::new(shutdown).args(["/s", "/t", "0"]).creation_flags(CREATE_NO_WINDOW))
        }
    }
}

#[cfg(windows)]
struct Hold(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Hold {
    fn new() -> Result<Self, String> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Power::{PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest};
        use windows_sys::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
        use windows_sys::Win32::System::Threading::{
            POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
        };

        // The reason shows in `powercfg /requests`.
        let mut reason: Vec<u16> = "Downloading files".encode_utf16().chain([0]).collect();
        let context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 { SimpleReasonString: reason.as_mut_ptr() },
        };
        // SAFETY: the context and its string are alive for the call, which copies them.
        let handle = unsafe { PowerCreateRequest(&context) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: the handle was just created and is closed once, here or in drop.
        if unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) } == 0 {
            let e = std::io::Error::last_os_error();
            // SAFETY: closed once, and never used again.
            unsafe { CloseHandle(handle) };
            return Err(e.to_string());
        }
        Ok(Self(handle))
    }
}

#[cfg(windows)]
impl Drop for Hold {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Power::{PowerClearRequest, PowerRequestSystemRequired};

        // SAFETY: the handle is this Hold's own, cleared and closed once.
        unsafe {
            PowerClearRequest(self.0, PowerRequestSystemRequired);
            CloseHandle(self.0);
        }
    }
}

#[cfg(target_os = "linux")]
const LOGIN: (&str, &str, &str) =
    ("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager");

#[cfg(target_os = "linux")]
fn act(action: Action) -> Result<(), String> {
    match action {
        Action::Sleep => login("Suspend"),
        // Through the desktop first, so apps get to save their work, as on the
        // other systems. logind alone would just close them.
        Action::ShutDown => desktop_shut_down().or_else(|_| login("PowerOff")),
    }
}

/// GNOME's or KDE's own shut down.
#[cfg(target_os = "linux")]
fn desktop_shut_down() -> zbus::Result<()> {
    let session = zbus::blocking::Connection::session()?;
    let gnome = ("org.gnome.SessionManager", "/org/gnome/SessionManager", "org.gnome.SessionManager", "Shutdown");
    let kde = ("org.kde.Shutdown", "/Shutdown", "org.kde.Shutdown", "logoutAndShutdown");
    let ask = |(name, path, interface, method): (&str, &str, &str, &str)| {
        session.call_method(Some(name), path, Some(interface), method, &()).map(|_| ())
    };
    ask(gnome).or_else(|_| ask(kde))
}

#[cfg(target_os = "linux")]
fn login(method: &str) -> Result<(), String> {
    let (name, path, interface) = LOGIN;
    let system = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
    // Interactive: if the system wants a password, the person can give one.
    system.call_method(Some(name), path, Some(interface), method, &(true,)).map(|_| ()).map_err(|e| e.to_string())
}

/// On Linux the desktop's portal keeps it awake: GNOME and KDE check it
/// before they suspend on their own. Without a portal, logind keeps itself
/// from suspending when idle; the person's own suspend still works.
#[cfg(target_os = "linux")]
enum Hold {
    Portal(zbus::blocking::Connection, zbus::zvariant::OwnedObjectPath),
    Login(#[allow(dead_code)] zbus::zvariant::OwnedFd),
}

#[cfg(target_os = "linux")]
const PORTAL: &str = "org.freedesktop.portal.Desktop";

#[cfg(target_os = "linux")]
impl Hold {
    fn new() -> Result<Self, String> {
        Self::portal().or_else(|portal| {
            Self::login().map_err(|login| format!("the desktop portal said \"{portal}\", and logind \"{login}\""))
        })
    }

    fn portal() -> zbus::Result<Self> {
        use std::collections::HashMap;
        use zbus::zvariant::Value;

        let session = zbus::blocking::Connection::session()?;
        let options = HashMap::from([("reason", Value::from("Downloading files"))]);
        // 4: suspending. Not 8, which would keep the screen on too.
        let reply = session.call_method(
            Some(PORTAL),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Inhibit"),
            "Inhibit",
            &("", 4u32, options),
        )?;
        let request = reply.body().deserialize()?;
        // It lasts while this connection does, so the Hold keeps it.
        Ok(Self::Portal(session, request))
    }

    fn login() -> zbus::Result<Self> {
        let (name, path, interface) = LOGIN;
        let system = zbus::blocking::Connection::system()?;
        // Idle only: "sleep" would also refuse the person's own suspend key.
        let reply = system.call_method(
            Some(name),
            path,
            Some(interface),
            "Inhibit",
            &("idle", "indir", "Downloading files", "block"),
        )?;
        // Held while this file stays open; closing it lets go.
        Ok(Self::Login(reply.body().deserialize()?))
    }
}

#[cfg(target_os = "linux")]
impl Drop for Hold {
    fn drop(&mut self) {
        if let Self::Portal(session, request) = self {
            let _ = session.call_method(
                Some(PORTAL),
                request.as_str(),
                Some("org.freedesktop.portal.Request"),
                "Close",
                &(),
            );
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn act(_: Action) -> Result<(), String> {
    Err("indir can't do that on this system".into())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
struct Hold;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
impl Hold {
    fn new() -> Result<Self, String> {
        Ok(Self)
    }
}
