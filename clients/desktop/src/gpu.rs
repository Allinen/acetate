//! Choosing the GPU to draw with, with a fallback: every adapter that can draw to the
//! window is tried in turn, and the first one that gives a device is used. A GPU can be
//! present but unusable (its memory taken by another program, a broken driver), and
//! eframe would otherwise stop at the first adapter it picks.
//!
//! The order: the integrated GPU first (a music player draws little), then a dedicated
//! one, then anything else, and the software renderer last. `WGPU_POWER_PREF=high` puts
//! the dedicated GPU first.

use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, wgpu};

pub fn options() -> WgpuConfiguration {
    let mut options = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        let device_descriptor = Arc::clone(&setup.device_descriptor);
        let preference = wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower);
        setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| {
            let mut candidates: Vec<&wgpu::Adapter> =
                adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
            candidates.sort_by_key(|a| rank(a.get_info().device_type, preference));
            let mut failures = Vec::new();
            for adapter in candidates {
                let info = adapter.get_info();
                // Opened to see that it can be, then let go: eframe opens its own.
                match pollster::block_on(adapter.request_device(&device_descriptor(adapter))) {
                    Ok(_) => return Ok(adapter.clone()),
                    Err(e) => {
                        eprintln!("GPU {} ({:?}) can't be used: {e}; trying the next one", info.name, info.backend);
                        failures.push(format!("{}: {e}", info.name));
                    }
                }
            }
            Err(if failures.is_empty() {
                "no GPU can draw to the window".to_owned()
            } else {
                format!("no GPU could be used ({})", failures.join("; "))
            })
        }));
    }
    options
}

fn rank(kind: wgpu::DeviceType, preference: wgpu::PowerPreference) -> u8 {
    use wgpu::DeviceType::*;
    let high = preference == wgpu::PowerPreference::HighPerformance;
    match kind {
        IntegratedGpu if !high => 0,
        DiscreteGpu if high => 0,
        IntegratedGpu | DiscreteGpu => 1,
        VirtualGpu => 2,
        Other => 3,
        Cpu => 4,
    }
}
