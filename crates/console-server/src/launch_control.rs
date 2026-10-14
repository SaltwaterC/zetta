//! Clears inherited Ctrl+C suppression in our suspended console child.
//! A pseudoconsole attribute does not reset ConsoleFlags like CREATE_NEW_CONSOLE
//! does. Editing only the owned child's process parameters keeps the embedding
//! process's handlers intact and avoids a bootstrap executable per console.
use std::ffi::c_void;
use std::io;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};

#[repr(C)]
#[derive(Default)]
struct ProcessBasicInformation {
    reserved: usize,
    peb: usize,
    reserved2: [usize; 2],
    pid: usize,
    reserved3: usize,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

pub(crate) fn enable_ctrl_c(process: HANDLE) -> io::Result<()> {
    let mut information = ProcessBasicInformation::default();
    query(process, 0, &mut information)?;
    reset_flags(process, information.peb, size_of::<usize>())?;
    // A 32-bit client on 64-bit Windows uses its WoW64 process parameters.
    #[cfg(target_pointer_width = "64")]
    {
        let mut wow64_peb = 0usize;
        query(process, 26, &mut wow64_peb)?;
        if wow64_peb != 0 {
            reset_flags(process, wow64_peb, 4)?;
        }
    }
    Ok(())
}

fn query<T>(process: HANDLE, class: u32, information: &mut T) -> io::Result<()> {
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            class,
            (information as *mut T).cast(),
            size_of::<T>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status < 0 {
        return Err(io::Error::other(format!(
            "reading the suspended console child's PEB: {status:#010x}"
        )));
    }
    Ok(())
}

fn reset_flags(process: HANDLE, peb: usize, pointer_size: usize) -> io::Result<()> {
    // PEB.ProcessParameters follows four pointer-sized slots (including the
    // padded initial flags). RTL_USER_PROCESS_PARAMETERS begins with four
    // ULONGs, ConsoleHandle, then ConsoleFlags. These layouts are shared by
    // x86, x64 and ARM64, with their respective pointer widths.
    let pointer = read::<8>(process, peb + 4 * pointer_size, pointer_size)?;
    let parameters = u64::from_le_bytes(pointer) as usize;
    let address = parameters + 16 + pointer_size;
    let flags = u32::from_le_bytes(read::<4>(process, address, 4)?);
    if flags & 1 == 0 {
        return Ok(());
    }
    let enabled = (flags & !1).to_le_bytes();
    let mut written = 0;
    let ok = unsafe {
        WriteProcessMemory(
            process,
            address as *mut c_void,
            enabled.as_ptr().cast(),
            enabled.len(),
            &mut written,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if written != enabled.len() {
        return Err(io::Error::other("incomplete console control flag write"));
    }
    Ok(())
}

fn read<const N: usize>(process: HANDLE, address: usize, length: usize) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    let mut received = 0;
    let ok = unsafe {
        ReadProcessMemory(
            process,
            address as *const c_void,
            bytes.as_mut_ptr().cast(),
            length,
            &mut received,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if received != length {
        return Err(io::Error::other(
            "incomplete console process parameter read",
        ));
    }
    Ok(bytes)
}
