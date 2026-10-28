//! Window capture through WGC, including occluded windows. No desktop pixels or
//! on-disk image files are needed. Every resource closes on success and failure.
use super::Deadline;
use std::{
    thread,
    time::{Duration, Instant},
};
use windows::{
    core::Interface,
    Graphics::{
        Capture::*,
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{Direct3D::*, Direct3D11::*, Dxgi::IDXGIDevice},
        System::WinRT::{Direct3D11::*, Graphics::Capture::IGraphicsCaptureItemInterop},
    },
};

fn err(e: impl std::fmt::Display) -> String {
    format!("Window capture unavailable: {e}")
}
pub fn window_png(hwnd: HWND, deadline: &Deadline) -> Result<(Vec<u8>, u32, u32), String> {
    deadline.check()?;
    if !GraphicsCaptureSession::IsSupported().map_err(err)? {
        return Err("Windows Graphics Capture is unsupported on this system".into());
    }
    unsafe {
        let factory: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(err)?;
        let item: GraphicsCaptureItem = factory.CreateForWindow(hwnd).map_err(err)?;
        let size = item.Size().map_err(err)?;
        if size.Width <= 0
            || size.Height <= 0
            || size.Width as u64 * size.Height as u64 > 32_000_000
        {
            return Err("Window capture dimensions are invalid or too large".into());
        }
        let mut device = None;
        let mut context = None;
        // WARP also supports desktops without a hardware D3D adapter (e.g. RDP).
        let create = |driver, device: &mut _, context: &mut _| {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(device),
                None,
                Some(context),
            )
        };
        create(D3D_DRIVER_TYPE_HARDWARE, &mut device, &mut context)
            .or_else(|_| create(D3D_DRIVER_TYPE_WARP, &mut device, &mut context))
            .map_err(err)?;
        let device = device.ok_or("D3D device unavailable")?;
        let context = context.ok_or("D3D context unavailable")?;
        let dxgi: IDXGIDevice = device.cast().map_err(err)?;
        let direct: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .and_then(|v| v.cast())
            .map_err(err)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(err)?;
        struct Pool(Direct3D11CaptureFramePool);
        impl Drop for Pool {
            fn drop(&mut self) {
                let _ = self.0.Close();
            }
        }
        let pool = Pool(pool);
        let session = pool.0.CreateCaptureSession(&item).map_err(err)?;
        struct Session(GraphicsCaptureSession);
        impl Drop for Session {
            fn drop(&mut self) {
                let _ = self.0.Close();
            }
        }
        let session = Session(session);
        let _ = session.0.SetIsCursorCaptureEnabled(false);
        session.0.StartCapture().map_err(err)?;
        let started = Instant::now();
        let frame = loop {
            deadline.check()?;
            if let Ok(frame) = pool.0.TryGetNextFrame() {
                break frame;
            }
            if started.elapsed() > Duration::from_millis(1500) {
                return Err(
                    "Window produced no capture frame (minimized, protected, or unavailable)"
                        .into(),
                );
            }
            thread::sleep(Duration::from_millis(10));
        };
        struct Frame(Direct3D11CaptureFrame);
        impl Drop for Frame {
            fn drop(&mut self) {
                let _ = self.0.Close();
            }
        }
        let frame = Frame(frame);
        let content = frame.0.ContentSize().map_err(err)?;
        if content != size {
            return Err("Window resized during capture; observe again".into());
        }
        let access: IDirect3DDxgiInterfaceAccess =
            frame.0.Surface().and_then(|s| s.cast()).map_err(err)?;
        let source: ID3D11Texture2D = access.GetInterface().map_err(err)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        source.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.MiscFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        device
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .map_err(err)?;
        let staging = staging.ok_or("Cannot allocate capture staging texture")?;
        context.CopyResource(&staging, &source);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(err)?;
        struct Map<'a>(&'a ID3D11DeviceContext, &'a ID3D11Texture2D);
        impl Drop for Map<'_> {
            fn drop(&mut self) {
                unsafe {
                    self.0.Unmap(self.1, 0);
                }
            }
        }
        let _map = Map(&context, &staging);
        let (width, height) = (size.Width as u32, size.Height as u32);
        if desc.Width < width
            || desc.Height < height
            || mapped.RowPitch < width * 4
            || mapped.pData.is_null()
        {
            return Err("Invalid captured texture layout".into());
        }
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height as usize {
            deadline.check()?;
            let row = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                width as usize * 4,
            );
            for (src, dst) in row
                .chunks_exact(4)
                .zip(rgba[y * width as usize * 4..(y + 1) * width as usize * 4].chunks_exact_mut(4))
            {
                dst.copy_from_slice(&[src[2], src[1], src[0], 255]);
            }
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(err)?;
            writer.write_image_data(&rgba).map_err(err)?;
        }
        Ok((bytes, width, height))
    }
}
