//! Dünne Windows-Aufrufe: Laufwerke, Adminrechte, Papierkorb, Explorer, Prozesse, Befehle.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Storage::FileSystem::{
    FileAttributeTagInfo, GetDiskFreeSpaceExW, GetDriveTypeW, GetFileInformationByHandleEx,
    GetLogicalDriveStringsW, GetVolumeInformationW, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::Shell::{
    IsUserAnAdmin, SHEmptyRecycleBinW, SHFileOperationW, SHQueryRecycleBinW, ShellExecuteW,
    SHFILEOPSTRUCTW, SHQUERYRBINFO,
};

const OHNE_FENSTER: u32 = 0x0800_0000;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub struct Laufwerk {
    pub wurzel: String,
    pub name: String,
    pub groesse: u64,
    pub frei: u64,
}

pub fn laufwerke() -> Vec<Laufwerk> {
    let mut buf = [0u16; 512];
    let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
    buf[..n.min(buf.len())]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let wurzel = String::from_utf16_lossy(s);
            let ws = w(&wurzel);
            let typ = unsafe { GetDriveTypeW(ws.as_ptr()) };
            // 2 = Wechseldatenträger, 3 = Festplatte; Netz/CD nicht
            if typ != 2 && typ != 3 {
                return None;
            }
            let (groesse, frei) = platz(&wurzel)?;
            let mut name = [0u16; 261];
            let ok = unsafe {
                GetVolumeInformationW(
                    ws.as_ptr(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            } != 0;
            let name = if ok {
                String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(0)])
            } else {
                String::new()
            };
            let name = if !name.is_empty() {
                name
            } else if typ == 2 {
                "USB-Stick / Wechseldatenträger".into()
            } else {
                "Lokaler Datenträger".into()
            };
            Some(Laufwerk {
                wurzel,
                name,
                groesse,
                frei,
            })
        })
        .collect()
}

/// (Größe, frei) eines Laufwerks, z. B. "C:\".
pub fn platz(wurzel: &str) -> Option<(u64, u64)> {
    let (mut frei, mut gesamt, mut _f) = (0u64, 0u64, 0u64);
    let ok =
        unsafe { GetDiskFreeSpaceExW(w(wurzel).as_ptr(), &mut frei, &mut gesamt, &mut _f) } != 0;
    (ok && gesamt > 0).then_some((gesamt, frei))
}

pub fn ist_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// Startet Speichermonitor mit Adminrechten neu (UAC-Abfrage). true = neuer Prozess läuft.
pub fn als_admin_neu_starten() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = w(&exe.to_string_lossy());
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            w("runas").as_ptr(),
            exe.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    r as usize > 32
}

pub fn im_explorer_zeigen(p: &Path) {
    let _ = Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", p.display()))
        .spawn();
}

/// Verschiebt in den Papierkorb. Kann ein Laufwerk keinen Papierkorb, warnt Windows
/// selbst, bevor endgültig gelöscht würde (FOF_WANTNUKEWARNING).
pub fn in_papierkorb(p: &Path) -> bool {
    const FO_DELETE: u32 = 3;
    const FOF_NOCONFIRMATION: u16 = 0x10;
    const FOF_ALLOWUNDO: u16 = 0x40;
    const FOF_WANTNUKEWARNING: u16 = 0x4000;
    let mut von: Vec<u16> = p.to_string_lossy().encode_utf16().collect();
    von.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: von.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    r == 0 && op.fAnyOperationsAborted == 0 && !p.exists()
}

pub fn papierkorb_groesse() -> u64 {
    let mut i = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        i64Size: 0,
        i64NumItems: 0,
    };
    if unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut i) } < 0 {
        return 0;
    }
    i.i64Size.max(0) as u64
}

pub fn papierkorb_leeren() {
    // SHERB_NOCONFIRMATION | NOPROGRESSUI | NOSOUND – bestätigt hat der Nutzer schon in Speichermonitor
    unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), 1 | 2 | 4) };
}

/// Läuft ein Prozess mit diesem Exe-Namen (z. B. "chrome.exe")?
pub fn laeuft(exe: &str) -> bool {
    unsafe {
        let h = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if h.is_null() || h as isize == -1 {
            return false;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut gefunden = false;
        let mut ok = Process32FirstW(h, &mut e) != 0;
        while ok && !gefunden {
            let n = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            gefunden = String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case(exe);
            ok = Process32NextW(h, &mut e) != 0;
        }
        CloseHandle(h);
        gefunden
    }
}

/// Reparse-Tag eines Eintrags, ohne ihm zu folgen (0 = kein Reparse-Point). None = nicht lesbar.
pub fn reparse_tag(p: &Path) -> Option<u32> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    let f = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(p)
        .ok()?;
    let mut i = FILE_ATTRIBUTE_TAG_INFO {
        FileAttributes: 0,
        ReparseTag: 0,
    };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            f.as_raw_handle(),
            FileAttributeTagInfo,
            &mut i as *mut _ as *mut _,
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } != 0;
    ok.then_some(i.ReparseTag)
}

/// Führt ein Programm aus System32 (z. B. "dism.exe") unsichtbar aus und wartet. true = Exit-Code 0.
pub fn befehl(prog: &str, args: &[&str]) -> bool {
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    Command::new(Path::new(&win).join("System32").join(prog))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(OHNE_FENSTER)
        .status()
        .is_ok_and(|s| s.success())
}
