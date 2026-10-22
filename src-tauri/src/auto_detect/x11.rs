//! X11/XWayland capture backend.
//!
//! D2R under Proton is an XWayland client, even when the desktop session is
//! Wayland. XGetImage on the game window works without an xdg-desktop-portal
//! prompt, including when the window is on another workspace.

use std::fmt;
use std::io::Cursor;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Default for Region {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GameWindowInfo {
    pub id: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl ImageData {
    pub fn mean_brightness(&self) -> f32 {
        if self.rgb.is_empty() {
            return 0.0;
        }
        let total: u64 = self.rgb.iter().map(|value| u64::from(*value)).sum();
        (total as f64 / self.rgb.len() as f64) as f32
    }

    pub fn to_png_base64(&self) -> Result<String, CaptureError> {
        let image = image::RgbImage::from_raw(self.width, self.height, self.rgb.clone())
            .ok_or_else(|| CaptureError::Encode("invalid RGB buffer".into()))?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|error| CaptureError::Encode(error.to_string()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        Ok(format!("data:image/png;base64,{encoded}"))
    }
}

#[derive(Debug)]
pub enum CaptureError {
    Connect(String),
    WindowNotFound,
    X11(String),
    RegionOutOfBounds,
    UnsupportedFormat(usize),
    Encode(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::Connect(message) => {
                write!(formatter, "cannot connect to X11: {message}")
            }
            CaptureError::WindowNotFound => write!(formatter, "Diablo II window not found"),
            CaptureError::X11(message) => write!(formatter, "X11 error: {message}"),
            CaptureError::RegionOutOfBounds => {
                write!(formatter, "capture region is outside the game window")
            }
            CaptureError::UnsupportedFormat(bytes) => {
                write!(
                    formatter,
                    "unsupported pixel format ({bytes} bytes per pixel)"
                )
            }
            CaptureError::Encode(message) => write!(formatter, "cannot encode preview: {message}"),
        }
    }
}

impl std::error::Error for CaptureError {}

pub trait CaptureBackend {
    fn find_game_window(&self) -> Result<GameWindowInfo, CaptureError>;
    fn capture_region(
        &self,
        window: &GameWindowInfo,
        region: Region,
    ) -> Result<ImageData, CaptureError>;
}

pub struct X11Backend {
    connection: RustConnection,
    root: Window,
    net_client_list: u32,
    net_wm_name: u32,
    utf8_string: u32,
}

impl X11Backend {
    pub fn new() -> Result<Self, CaptureError> {
        let (connection, screen_number) =
            x11rb::connect(None).map_err(|error| CaptureError::Connect(error.to_string()))?;
        let root = connection.setup().roots[screen_number].root;
        let net_client_list = intern_atom(&connection, b"_NET_CLIENT_LIST")?;
        let net_wm_name = intern_atom(&connection, b"_NET_WM_NAME")?;
        let utf8_string = intern_atom(&connection, b"UTF8_STRING")?;
        Ok(Self {
            connection,
            root,
            net_client_list,
            net_wm_name,
            utf8_string,
        })
    }

    fn find_by_client_list(&self) -> Result<Option<Window>, CaptureError> {
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                self.net_client_list,
                AtomEnum::WINDOW,
                0,
                u32::MAX,
            )
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        if let Some(values) = reply.value32() {
            for window in values {
                if self.is_game_window(window) {
                    return Ok(Some(window));
                }
            }
        }
        Ok(None)
    }

    fn find_in_tree(&self, window: Window, depth: u32) -> Result<Option<Window>, CaptureError> {
        if self.is_game_window(window) {
            return Ok(Some(window));
        }
        if depth == 0 {
            return Ok(None);
        }
        let tree = self
            .connection
            .query_tree(window)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        for child in tree.children {
            if let Some(found) = self.find_in_tree(child, depth - 1)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    fn is_game_window(&self, window: Window) -> bool {
        let title = self.window_title(window).unwrap_or_default().to_lowercase();
        let class = self.window_class(window).unwrap_or_default().to_lowercase();
        (title.contains("diablo ii") && title.contains("resurrected"))
            || (class == "steam_app_default" && title.contains("diablo"))
    }

    fn window_title(&self, window: Window) -> Result<String, CaptureError> {
        let reply = self
            .connection
            .get_property(false, window, self.net_wm_name, self.utf8_string, 0, 1024)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        if !reply.value.is_empty() {
            return Ok(String::from_utf8_lossy(&reply.value).to_string());
        }
        let reply = self
            .connection
            .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        Ok(String::from_utf8_lossy(&reply.value).to_string())
    }

    fn window_class(&self, window: Window) -> Result<String, CaptureError> {
        let reply = self
            .connection
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        let parts: Vec<&[u8]> = reply.value.split(|byte| *byte == 0).collect();
        let value = parts
            .get(1)
            .or_else(|| parts.first())
            .copied()
            .unwrap_or_default();
        Ok(String::from_utf8_lossy(value).to_string())
    }

    fn window_info(&self, window: Window) -> Result<GameWindowInfo, CaptureError> {
        let geometry = self
            .connection
            .get_geometry(window)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        let position = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        Ok(GameWindowInfo {
            id: window,
            title: self.window_title(window).unwrap_or_default(),
            x: i32::from(position.dst_x),
            y: i32::from(position.dst_y),
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
        })
    }
}

impl CaptureBackend for X11Backend {
    fn find_game_window(&self) -> Result<GameWindowInfo, CaptureError> {
        if let Some(window) = self.find_by_client_list()? {
            return self.window_info(window);
        }
        if let Some(window) = self.find_in_tree(self.root, 3)? {
            return self.window_info(window);
        }
        Err(CaptureError::WindowNotFound)
    }

    fn capture_region(
        &self,
        window: &GameWindowInfo,
        region: Region,
    ) -> Result<ImageData, CaptureError> {
        let x = region.x.max(0);
        let y = region.y.max(0);
        let available_width = window.width.saturating_sub(x as u32);
        let available_height = window.height.saturating_sub(y as u32);
        let width = region.width.min(available_width);
        let height = region.height.min(available_height);
        if width == 0 || height == 0 {
            return Err(CaptureError::RegionOutOfBounds);
        }

        let reply = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                window.id,
                x as i16,
                y as i16,
                width as u16,
                height as u16,
                !0,
            )
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;

        let pixel_count = width as usize * height as usize;
        if pixel_count == 0 || reply.data.len() % pixel_count != 0 {
            return Err(CaptureError::UnsupportedFormat(reply.data.len()));
        }
        let bytes_per_pixel = reply.data.len() / pixel_count;
        if bytes_per_pixel != 3 && bytes_per_pixel != 4 {
            return Err(CaptureError::UnsupportedFormat(bytes_per_pixel));
        }

        let mut rgb = Vec::with_capacity(pixel_count * 3);
        for pixel in reply.data.chunks_exact(bytes_per_pixel) {
            rgb.push(pixel[2]);
            rgb.push(pixel[1]);
            rgb.push(pixel[0]);
        }

        Ok(ImageData { width, height, rgb })
    }
}

fn intern_atom(connection: &RustConnection, name: &[u8]) -> Result<u32, CaptureError> {
    connection
        .intern_atom(false, name)
        .map_err(x11_error)?
        .reply()
        .map_err(x11_error)
        .map(|reply| reply.atom)
}

fn x11_error(error: impl fmt::Display) -> CaptureError {
    CaptureError::X11(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_brightness_works() {
        let image = ImageData {
            width: 2,
            height: 1,
            rgb: vec![0, 0, 0, 255, 255, 255],
        };
        assert!((image.mean_brightness() - 127.5).abs() < 0.1);
    }

    #[test]
    fn png_preview_has_data_url_prefix() {
        let image = ImageData {
            width: 1,
            height: 1,
            rgb: vec![255, 0, 0],
        };
        let png = image.to_png_base64().expect("png encoding");
        assert!(png.starts_with("data:image/png;base64,"));
    }

    #[test]
    #[ignore = "requires a running D2R XWayland window"]
    fn captures_live_game_window() {
        let backend = X11Backend::new().expect("X11 connection");
        let window = backend.find_game_window().expect("D2R window");
        let image = backend
            .capture_region(
                &window,
                Region {
                    x: 1180,
                    y: 670,
                    width: 200,
                    height: 100,
                },
            )
            .expect("capture");
        assert_eq!(image.rgb.len(), 200 * 100 * 3);
        println!("window: {window:?}");
        println!("brightness: {:.2}", image.mean_brightness());
    }
}
