use windows::core::Interface;
use super::monitors;
use crate::config::CaptureConfig;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// DXGI Desktop Duplication을 통한 화면 캡처
pub struct ScreenCapture {
    _device: ID3D11Device, // keeps the adapter device alive
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
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

            let staging = create_staging(&device, width, height)?;

            Ok(Self { _device: device, context, duplication, staging, width, height })
        }
    }

    /// 프레임 캡처 → BGRA 픽셀 데이터 반환
    /// 반환: (데이터, 행 피치)
    pub fn capture_frame(&mut self) -> Result<(Vec<u8>, u32), CaptureError> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;

            // 다음 프레임 획득 (최대 100ms 대기)
            let result = self.duplication.AcquireNextFrame(100, &mut frame_info, &mut resource);

            match result {
                Ok(()) => {}
                Err(e) => {
                    let code = e.code();
                    if code == DXGI_ERROR_WAIT_TIMEOUT {
                        return Err(CaptureError::Timeout);
                    }
                    if code == DXGI_ERROR_ACCESS_LOST {
                        return Err(CaptureError::AccessLost);
                    }
                    return Err(CaptureError::Other(format!("AcquireNextFrame 실패: {}", e)));
                }
            }

            // 프레임 획득 성공 — 이후 모든 경로에서 ReleaseFrame 호출 보장
            let resource = match resource {
                Some(r) => r,
                None => {
                    let _ = self.duplication.ReleaseFrame();
                    return Err(CaptureError::Other("리소스 없음".into()));
                }
            };

            let texture: ID3D11Texture2D = match resource.cast() {
                Ok(t) => t,
                Err(e) => {
                    let _ = self.duplication.ReleaseFrame();
                    return Err(CaptureError::Other(format!("{}", e)));
                }
            };

            self.context.CopyResource(&self.staging, &texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = self.context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) {
                let _ = self.duplication.ReleaseFrame();
                return Err(CaptureError::Other(format!("Map 실패: {}", e)));
            }

            let pitch = mapped.RowPitch;
            let data_size = (pitch * self.height) as usize;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, data_size);
            let data = src.to_vec();

            self.context.Unmap(&self.staging, 0);
            let _ = self.duplication.ReleaseFrame();

            Ok((data, pitch))
        }
    }

    /// Re-create everything (AccessLost, resolution change, adapter change).
    pub fn reinitialize(&mut self, cfg: &CaptureConfig) -> Result<(), Box<dyn std::error::Error>> {
        *self = Self::new(cfg)?;
        Ok(())
    }
}

unsafe fn create_staging(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D, Box<dyn std::error::Error>> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
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
            Self::Timeout => write!(f, "프레임 타임아웃"),
            Self::AccessLost => write!(f, "Desktop Duplication 접근 손실"),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for CaptureError {}
