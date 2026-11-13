use anyhow::{Context, Result};
use glue5_gpu_d3d11::{D3D11Device, presentation::Presentation};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, RegisterClassW,
        SW_SHOW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_QUIT, WNDCLASSW,
        WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    },
};
use windows::core::w;

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
fn main() -> Result<()> {
    let cls =
        WNDCLASSW { lpfnWndProc: Some(window_proc), lpszClassName: w!("GLUE5GM45RustProbe"), ..Default::default() };
    anyhow::ensure!(unsafe { RegisterClassW(&cls) } != 0, "RegisterClassW failed");
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("GLUE5GM45RustProbe"),
            w!("GLUE5 GM45 / Rust D3D11 Present"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            720,
            420,
            None,
            None,
            None,
            None,
        )
    }
    .context("CreateWindowExW")?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    let device = D3D11Device::open_hardware().context("hardware D3D11 device")?;
    println!("DEVICE {:?}", device.feature_level());
    let presentation = unsafe { Presentation::new(&device, hwnd, 640, 360) }.context("GM45 DXGI swapchain")?;
    println!("SWAPCHAIN OK {}x{}", presentation.dimensions().0, presentation.dimensions().1);
    let mut completed = 0u32;
    for frame in 0..60u32 {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() } {
            if msg.message == WM_QUIT {
                anyhow::bail!("window closed before completion")
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let color = [frame as f32 / 60.0, 0.13, 0.36, 1.0];
        presentation.clear(&device, color);
        presentation.present(0)?;
        completed += 1;
    }
    presentation.unbind(&device);
    println!("PRESENT OK frames={completed}");
    drop(presentation);
    unsafe {
        DestroyWindow(hwnd).context("DestroyWindow")?;
    }
    Ok(())
}
