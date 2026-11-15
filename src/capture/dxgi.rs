use super::monitors;
use crate::config::CaptureConfig;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_SRV_DIMENSION_TEXTURE2D};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// Attach the calling thread to the current input desktop. Returns `false` while the secure
/// desktop (UAC prompt, lock screen) is showing: nothing can be captured then, and that is
/// not an error. Desktop Duplication only works from the desktop that currently has input.
pub fn attach_input_desktop() -> bool {
    use windows::Win32::System::StationsAndDesktops::{
        OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
    };
    const GENERIC_ALL: u32 = 0x1000_0000;
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(GENERIC_ALL)) {
            // The handle must stay open while the thread uses the desktop; it is a few
            // bytes and only re-opened after a desktop switch.
            Ok(h) => SetThreadDesktop(h).is_ok(),
            Err(_) => false,
        }
    }
}

/// Target width of the downscaled frame used for color extraction.
const SMALL_TARGET_WIDTH: u32 = 192;

/// Which mip level brings `width` down to at most `SMALL_TARGET_WIDTH`.
pub fn mip_level_for(width: u32) -> u32 {
    let mut level = 0;
    while (width >> level) > SMALL_TARGET_WIDTH && level < 8 {
        level += 1;
    }
    level
}

/// One captured (and possibly GPU-downscaled) BGRA frame.
#[derive(Clone)]
pub struct Frame {
    pub data: Vec<u8>,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
}

/// GPU downscale resources: desktop frame -> mip chain (averaged in linear light via
/// an sRGB view) -> read back only the small level.
struct GpuDownscale {
    mip_tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    level: u32,
    small_w: u32,
    small_h: u32,
    staging_small: ID3D11Texture2D,
    /// Desktop is FP16 scRGB (Windows HDR): read back floats and tone-map to SDR.
    hdr: bool,
}

/// DXGI Desktop Duplication screen capture.
pub struct ScreenCapture {
    _device: ID3D11Device, // keeps the adapter device alive
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    gpu: Option<GpuDownscale>,
    /// GPU resources are created lazily on the first frame (its format decides SDR vs HDR).
    gpu_wanted: bool,
    gpu_tried: bool,
    sdr_white_scale: f32,
    full_staging: Option<ID3D11Texture2D>,
    cache: Option<Frame>,
    pub width: u32,
    pub height: u32,
}

impl ScreenCapture {
    /// Desktop Duplication on the monitor chosen by config. The D3D11 device is created
    /// on the adapter that OWNS the output (required on hybrid-GPU laptops).
    pub fn new(cfg: &CaptureConfig) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            let monitors = monitors::enumerate()?;
            let m = monitors::select(&monitors, cfg.monitor, &cfg.monitor_name)
                .ok_or("requested monitor not found (run with --list-monitors)")?;
            if !m.attached {
                return Err(format!("monitor [{}] {} is not attached to the desktop", m.global_index, m.device_name).into());
            }

            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter: IDXGIAdapter1 = factory.EnumAdapters1(m.adapter_index)?;
            let adapter_base: IDXGIAdapter = adapter.cast()?;

            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter_base,
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or("D3D11 device creation failed")?;
            let context = context.ok_or("D3D11 context creation failed")?;

            let output: IDXGIOutput = adapter.EnumOutputs(m.output_index)?;
            let output1: IDXGIOutput1 = output.cast()?;
            let duplication = output1.DuplicateOutput(&device)?;

            let desc = output.GetDesc()?;
            let width = (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left) as u32;
            let height = (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top) as u32;

            log::info!(
                "capture init: {}x{} on [{}] {} / {}",
                width, height, m.global_index, m.adapter_name, m.device_name
            );

            Ok(Self {
                _device: device,
                context,
                duplication,
                gpu: None,
                gpu_wanted: cfg.gpu_downscale,
                gpu_tried: false,
                sdr_white_scale: cfg.hdr_sdr_white_nits / 80.0,
                full_staging: None,
                cache: None,
                width,
                height,
            })
        }
    }

    /// The most recent frame, for re-processing while the screen is static.
    pub fn last_frame(&self) -> Option<&Frame> {
        self.cache.as_ref()
    }

    /// Wait up to `timeout_ms` for a new frame.
    pub fn capture_frame(&mut self, timeout_ms: u32) -> Result<Frame, CaptureError> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;

            if let Err(e) = self.duplication.AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource) {
                let code = e.code();
                if code == DXGI_ERROR_WAIT_TIMEOUT {
                    return Err(CaptureError::Timeout);
                }
                if code == DXGI_ERROR_ACCESS_LOST {
                    return Err(CaptureError::AccessLost);
                }
                return Err(CaptureError::Other(format!("AcquireNextFrame failed: {}", e)));
            }

            // From here on every path must ReleaseFrame.
            let result = self.process_frame(&frame_info, resource);
            let _ = self.duplication.ReleaseFrame();
            result
        }
    }

    unsafe fn process_frame(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
    ) -> Result<Frame, CaptureError> {
        // Only the mouse pointer moved: the image is unchanged, reuse the cached frame.
        if info.AccumulatedFrames == 0 {
            if let Some(c) = &self.cache {
                return Ok(c.clone());
            }
        }

        let resource = resource.ok_or_else(|| CaptureError::Other("no resource".into()))?;
        let texture: ID3D11Texture2D = resource.cast().map_err(|e| CaptureError::Other(format!("{e}")))?;

        let mut tdesc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut tdesc);
        let hdr = tdesc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT;
        if !hdr && tdesc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(CaptureError::Other(format!("unsupported desktop format {:?}", tdesc.Format)));
        }

        // Lazily create GPU resources for the real desktop format; if the format changed
        // (HDR toggled) rebuild them.
        if self.gpu_wanted && (self.gpu.is_none() && !self.gpu_tried || self.gpu.as_ref().is_some_and(|g| g.hdr != hdr)) {
            self.gpu_tried = true;
            let device = self._device.clone();
            match create_gpu_downscale(&device, self.width, self.height, hdr) {
                Ok(g) => {
                    log::info!(
                        "GPU downscale active ({}): mip level {} -> {}x{} readback",
                        if hdr { "HDR FP16" } else { "SDR" }, g.level, g.small_w, g.small_h
                    );
                    self.gpu = Some(g);
                }
                Err(e) => {
                    log::warn!("GPU downscale unavailable ({e}); using full-frame CPU path");
                    self.gpu = None;
                }
            }
        }

        let frame = if let Some(gpu) = &self.gpu {
            self.context.CopySubresourceRegion(&gpu.mip_tex, 0, 0, 0, 0, &texture, 0, None);
            self.context.GenerateMips(&gpu.srv);
            self.context.CopySubresourceRegion(&gpu.staging_small, 0, 0, 0, 0, &gpu.mip_tex, gpu.level, None);
            if gpu.hdr {
                read_back_hdr(&self.context, &gpu.staging_small, gpu.small_w, gpu.small_h, self.sdr_white_scale)?
            } else {
                read_back(&self.context, &gpu.staging_small, gpu.small_w, gpu.small_h)?
            }
        } else {
            if hdr {
                return Err(CaptureError::Other(
                    "HDR desktop needs GPU downscale (enable capture.gpu_downscale) or turn Windows HDR off".into(),
                ));
            }
            if self.full_staging.is_none() {
                self.full_staging = Some(
                    create_staging(&self._device, self.width, self.height, DXGI_FORMAT_B8G8R8A8_UNORM)
                        .map_err(|e| CaptureError::Other(format!("{e}")))?,
                );
            }
            let staging = self.full_staging.as_ref().unwrap();
            self.context.CopyResource(staging, &texture);
            read_back(&self.context, staging, self.width, self.height)?
        };

        self.cache = Some(frame.clone());
        Ok(frame)
    }
}

unsafe fn read_back(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<Frame, CaptureError> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    context
        .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .map_err(|e| CaptureError::Other(format!("Map failed: {e}")))?;
    let pitch = mapped.RowPitch;
    let size = (pitch as usize) * (height as usize);
    let data = std::slice::from_raw_parts(mapped.pData as *const u8, size).to_vec();
    context.Unmap(staging, 0);
    Ok(Frame { data, pitch, width, height })
}

/// IEEE half -> f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let man = (h & 0x3FF) as f32;
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 => {
            if man == 0.0 { sign * f32::INFINITY } else { f32::NAN }
        }
        _ => sign * (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// scRGB linear value -> 8-bit sRGB, with SDR white at `white_scale` (nits/80).
pub fn hdr_to_srgb8(v: f32, white_scale: f32) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    let x = (v / white_scale.max(0.1)).clamp(0.0, 1.0);
    let e = if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    (e * 255.0).round().clamp(0.0, 255.0) as u8
}

unsafe fn read_back_hdr(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    width: u32,
    height: u32,
    white_scale: f32,
) -> Result<Frame, CaptureError> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    context
        .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .map_err(|e| CaptureError::Other(format!("Map failed: {e}")))?;
    let src_pitch = mapped.RowPitch as usize;
    let src = std::slice::from_raw_parts(mapped.pData as *const u8, src_pitch * height as usize);
    let out_pitch = width as usize * 4;
    let mut data = vec![255u8; out_pitch * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let o = y * src_pitch + x * 8;
            let ch = |i: usize| f16_to_f32(u16::from_le_bytes([src[o + i * 2], src[o + i * 2 + 1]]));
            let d = y * out_pitch + x * 4;
            data[d] = hdr_to_srgb8(ch(2), white_scale); // B
            data[d + 1] = hdr_to_srgb8(ch(1), white_scale); // G
            data[d + 2] = hdr_to_srgb8(ch(0), white_scale); // R
        }
    }
    context.Unmap(staging, 0);
    Ok(Frame { data, pitch: out_pitch as u32, width, height })
}

unsafe fn create_gpu_downscale(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    hdr: bool,
) -> Result<GpuDownscale, Box<dyn std::error::Error>> {
    // SDR: TYPELESS storage viewed as sRGB so GenerateMips averages in linear light.
    // HDR: FP16 scRGB is already linear.
    let (tex_fmt, view_fmt, stage_fmt) = if hdr {
        (DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R16G16B16A16_FLOAT)
    } else {
        (DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8A8_UNORM)
    };
    let level = mip_level_for(width);
    let small_w = (width >> level).max(1);
    let small_h = (height >> level).max(1);

    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: level + 1,
        ArraySize: 1,
        Format: tex_fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
    };
    let mut mip_tex = None;
    device.CreateTexture2D(&desc, None, Some(&mut mip_tex))?;
    let mip_tex = mip_tex.ok_or("mip texture creation failed")?;

    let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: view_fmt,
        ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: level + 1 },
        },
    };
    let mut srv = None;
    device.CreateShaderResourceView(&mip_tex, Some(&srv_desc), Some(&mut srv))?;
    let srv = srv.ok_or("SRV creation failed")?;

    let staging_small = create_staging(device, small_w, small_h, stage_fmt)?;

    Ok(GpuDownscale { mip_tex, srv, level, small_w, small_h, staging_small, hdr })
}

unsafe fn create_staging(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D, Box<dyn std::error::Error>> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    device.CreateTexture2D(&desc, None, Some(&mut staging))?;
    Ok(staging.ok_or("staging texture creation failed")?)
}

#[derive(Debug)]
pub enum CaptureError {
    Timeout,
    AccessLost,
    Other(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "frame timeout"),
            Self::AccessLost => write!(f, "desktop duplication access lost"),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for CaptureError {}

#[cfg(test)]
mod tests {
    use super::{f16_to_f32, hdr_to_srgb8, mip_level_for};

    #[test]
    fn half_floats() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0x4000), 2.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert!(f16_to_f32(0x7C00).is_infinite());
        assert!(f16_to_f32(0x7E00).is_nan());
        assert!((f16_to_f32(0x0001) - 5.96e-8).abs() < 1e-9); // smallest subnormal
    }

    #[test]
    fn hdr_tone_map() {
        assert_eq!(hdr_to_srgb8(0.0, 3.0), 0);
        assert_eq!(hdr_to_srgb8(3.0, 3.0), 255); // SDR white
        assert_eq!(hdr_to_srgb8(30.0, 3.0), 255); // HDR highlight clamps
        assert_eq!(hdr_to_srgb8(-1.0, 3.0), 0);
        assert_eq!(hdr_to_srgb8(f32::NAN, 3.0), 0);
        assert_eq!(hdr_to_srgb8(0.5, 1.0), 188); // linear 0.5 -> sRGB ~188
    }

    #[test]
    fn mip_levels() {
        assert_eq!(mip_level_for(2560), 4); // 160
        assert_eq!(mip_level_for(1920), 4); // 120
        assert_eq!(mip_level_for(3840), 5); // 120
        assert_eq!(mip_level_for(1280), 3); // 160
        assert_eq!(mip_level_for(192), 0);
        assert_eq!(mip_level_for(100), 0);
        assert_eq!(mip_level_for(u32::MAX), 8);
    }
}
