//! The D3D12 Agility SDK: a newer `D3D12Core.dll` made the runtime of the
//! whole process before the first device, so that what Windows' own runtime
//! lacks is there: `ID3D12Device13::OpenExistingHeapFromAddress1`, which
//! `VICTORY_INPLACE` needs to let the card read title memory where it lies
//! (no second copy in the PC's memory), refused on Windows 11 23H2 without it.
//!
//! The DLL is looked for in `VICTORY_D3D12_DIR`, then in `D3D12\` beside
//! victory.exe (Microsoft's `Microsoft.Direct3D.D3D12` package,
//! `build\native\bin\x64`). Without one nothing changes; `VICTORY_AGILITY=0`
//! leaves it unused. After 101Leafy/victory-leafy and
//! bittertea0323/cool-fox-archive's `agility.rs`: the runtime is switched
//! through `ID3D12SDKConfiguration1` instead of exports of the executable.

use std::path::PathBuf;

use windows::Win32::Graphics::Direct3D12::{
    CLSID_D3D12SDKConfiguration, D3D12_DEVICE_FACTORY_FLAG_ALLOW_RETURNING_EXISTING_DEVICE, D3D12GetInterface,
    ID3D12DeviceFactory, ID3D12SDKConfiguration1,
};
use windows::core::PCSTR;

/// Makes the Agility SDK's runtime the process's, once; the version used.
pub(crate) fn load() -> Option<u32> {
    static DONE: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *DONE.get_or_init(|| {
        if std::env::var_os("VICTORY_AGILITY").is_some_and(|v| v == "0") {
            return None;
        }
        let dir = sdk_dir()?;
        let version = match std::fs::read(dir.join("D3D12Core.dll")).map_err(|e| e.to_string()).and_then(|d| sdk_version(&d)) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), "D3D12 Agility SDK not used: {e}");
                return None;
            }
        };
        let mut path = dir.to_string_lossy().into_owned();
        if !path.ends_with('\\') {
            path.push('\\');
        }
        path.push('\0');
        match apply(version, &path) {
            Ok(()) => {
                tracing::info!(version, dir = %dir.display(), "D3D12 Agility SDK loaded");
                Some(version)
            }
            Err(e) => {
                tracing::warn!(version, dir = %dir.display(), "D3D12 Agility SDK not used: {e}");
                None
            }
        }
    })
}

fn sdk_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("VICTORY_D3D12_DIR").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => std::env::current_exe().ok()?.parent()?.join("D3D12"),
    };
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    dir.join("D3D12Core.dll").is_file().then_some(dir)
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// The value of the `D3D12SDKVersion` export of a PE32+ image (its bytes).
fn sdk_version(d: &[u8]) -> Result<u32, String> {
    let pe = u32_at(d, 0x3c).ok_or("not a PE image")? as usize;
    if d.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("not a PE image".into());
    }
    let sections = u16_at(d, pe + 6).ok_or("short")? as usize;
    let optional = pe + 24;
    let optional_len = u16_at(d, pe + 20).ok_or("short")? as usize;
    if u16_at(d, optional) != Some(0x20b) {
        return Err("not PE32+".into());
    }
    let export_rva = u32_at(d, optional + 112).ok_or("short")?;
    let table = optional + optional_len;
    let offset = |rva: u32| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let (va, size, raw) = (u32_at(d, s + 12)?, u32_at(d, s + 8)?.max(u32_at(d, s + 16)?), u32_at(d, s + 20)?);
            (rva >= va && rva < va + size).then(|| (rva - va + raw) as usize)
        })
    };
    let e = offset(export_rva).ok_or("no export table")?;
    let (names, functions, ordinals) = (u32_at(d, e + 24).ok_or("short")?, u32_at(d, e + 28).ok_or("short")?, u32_at(d, e + 36).ok_or("short")?);
    let (name_table, ordinal_table) = (offset(u32_at(d, e + 32).ok_or("short")?).ok_or("bad")?, offset(ordinals).ok_or("bad")?);
    let function_table = offset(functions).ok_or("bad")?;
    for i in 0..names as usize {
        let name = offset(u32_at(d, name_table + i * 4).ok_or("short")?).ok_or("bad")?;
        if d.get(name..name + 16) == Some(b"D3D12SDKVersion\0") {
            let ordinal = u16_at(d, ordinal_table + i * 2).ok_or("short")? as usize;
            let rva = u32_at(d, function_table + ordinal * 4).ok_or("short")?;
            return u32_at(d, offset(rva).ok_or("bad")?).ok_or_else(|| "D3D12SDKVersion unreadable".into());
        }
    }
    Err("no D3D12SDKVersion export".into())
}

fn apply(version: u32, path: &str) -> windows::core::Result<()> {
    // SAFETY: `path` ends in a NUL; the factory is kept for the life of the
    // process, since the global state it applied refers to it.
    unsafe {
        let mut config: Option<ID3D12SDKConfiguration1> = None;
        D3D12GetInterface(&CLSID_D3D12SDKConfiguration, &mut config)?;
        let config = config.ok_or_else(windows::core::Error::empty)?;
        let factory: ID3D12DeviceFactory = config.CreateDeviceFactory(version, PCSTR(path.as_ptr()))?;
        factory.SetFlags(D3D12_DEVICE_FACTORY_FLAG_ALLOW_RETURNING_EXISTING_DEVICE)?;
        factory.ApplyToGlobalState()?;
        std::mem::forget(factory);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The version is read from the package's DLL where it is there
    /// (`VICTORY_D3D12_DIR`).
    #[test]
    fn version_of_the_package_dll() {
        let Some(dir) = std::env::var_os("VICTORY_D3D12_DIR") else { return };
        let dll = std::path::Path::new(&dir).join("D3D12Core.dll");
        let Ok(data) = std::fs::read(dll) else { return };
        assert_eq!(super::sdk_version(&data), Ok(620));
        assert!(super::sdk_version(b"MZ not a pe").is_err());
    }
}
