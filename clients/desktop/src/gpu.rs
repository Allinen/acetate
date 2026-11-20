//! Choosing the GPU to draw with. A GPU can be present but unusable (its memory taken by
//! another program, a broken driver), and eframe would stop at the one it picks, so each
//! is tried in turn and the first that opens is used.
//!
//! Opening isn't all: the window is shown on a screen, and a GPU without that screen has
//! to hand its frames over to the one with it, which not every pairing supports (a
//! dedicated GPU drawing for an integrated one's screen works, as on laptops; the other
//! way around may not, and the window can't be drawn at all). So the order is:
//!
//! - where it can be seen which GPUs the screens are plugged into (Linux): those first,
//!   then the software renderer (slow, but it draws anywhere), then the rest;
//! - elsewhere: a dedicated GPU, an integrated one, anything else, the software renderer
//!   last (Windows hands frames between GPUs itself).

use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, wgpu};

pub fn options() -> WgpuConfiguration {
    let mut options = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        let device_descriptor = Arc::clone(&setup.device_descriptor);
        setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| {
            let screens = gpus_with_a_screen();
            let mut candidates: Vec<&wgpu::Adapter> =
                adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
            candidates.sort_by_key(|a| rank(&a.get_info(), &screens));
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

/// Where the adapter is tried, lowest first.
fn rank(info: &wgpu::AdapterInfo, screens: &[Gpu]) -> u8 {
    use wgpu::DeviceType::*;
    if screens.is_empty() {
        return match info.device_type {
            DiscreteGpu => 0,
            IntegratedGpu => 1,
            VirtualGpu => 2,
            Other => 3,
            Cpu => 4,
        };
    }
    if screens.iter().any(|gpu| gpu.is(info)) {
        0
    } else if info.device_type == Cpu {
        1
    } else {
        2
    }
}

/// A GPU as the system lists it.
struct Gpu {
    /// PCI address, as `0000:01:00.0`.
    address: String,
    vendor: u32,
    device: u32,
}

impl Gpu {
    fn is(&self, info: &wgpu::AdapterInfo) -> bool {
        if info.device_pci_bus_id.is_empty() {
            // Not every driver gives the address; the model then (two of the same are
            // taken for one).
            info.vendor == self.vendor && info.device == self.device
        } else {
            info.device_pci_bus_id == self.address
        }
    }
}

/// The GPUs with a screen plugged in, from `/sys/class/drm`: a connector `card0-DP-1`
/// whose `status` is `connected` belongs to the GPU at `card0/device`. Empty elsewhere
/// than Linux, or when it can't be read.
fn gpus_with_a_screen() -> Vec<Gpu> {
    let drm = std::path::Path::new("/sys/class/drm");
    let Ok(entries) = std::fs::read_dir(drm) else { return Vec::new() };
    let mut gpus: Vec<Gpu> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((card, _connector)) = name.split_once('-') else { continue };
        if !card.starts_with("card")
            || !std::fs::read_to_string(entry.path().join("status")).is_ok_and(|s| s.trim() == "connected")
        {
            continue;
        }
        let device = drm.join(card).join("device");
        let id = |file: &str| {
            std::fs::read_to_string(device.join(file))
                .ok()
                .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        };
        if let Ok(path) = std::fs::canonicalize(&device)
            && let Some(address) = path.file_name()
            && let (Some(vendor), Some(device)) = (id("vendor"), id("device"))
        {
            let address = address.to_string_lossy().into_owned();
            if !gpus.iter().any(|gpu| gpu.address == address) {
                gpus.push(Gpu { address, vendor, device });
            }
        }
    }
    gpus
}
