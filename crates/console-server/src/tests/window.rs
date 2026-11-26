use super::*;
use std::sync::Mutex;

#[test]
fn the_window_is_hidden_reports_show_window_and_goes_away_cleanly() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    let window = PseudoWindow::create(
        None,
        Arc::new(move |shown| record.lock().unwrap().push(shown)),
    )
    .unwrap();
    let hwnd = window.hwnd() as HWND;
    // SAFETY: IsWindow takes a window handle by value and accepts any value.
    assert!(unsafe { IsWindow(hwnd) } != 0);
    assert_eq!(
        // SAFETY: hwnd is the window `window` keeps alive until dropped.
        unsafe { IsWindowVisible(hwnd) },
        0,
        "a console window starts hidden"
    );
    // SAFETY: hwnd is the window `window` keeps alive until dropped.
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        ShowWindow(hwnd, SW_HIDE);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    // Showing reports twice (WM_SHOWWINDOW and WM_SIZE), as OpenConsole does.
    while seen.lock().unwrap().last() != Some(&false) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        (seen.first(), seen.last()),
        (Some(&true), Some(&false)),
        "{seen:?}"
    );
    drop(window);
    // SAFETY: IsWindow accepts any handle value, including a destroyed one.
    assert_eq!(unsafe { IsWindow(hwnd) }, 0);
}
