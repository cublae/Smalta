//! wp_viewporter: a buffer shown at a size other than its own.
//!
//! The click catcher behind a popup covers the whole screen and shows
//! nothing; one transparent pixel stretched over the screen does that as
//! well as a screen's worth of them, eleven megabytes on a wide monitor.

use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, Proxy, QueueHandle, globals::GlobalList};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use crate::App;

pub struct ViewporterData;
pub struct ViewportData;

impl Dispatch2<WpViewporter, App> for ViewporterData {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpViewport, App> for ViewportData {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

pub fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<WpViewporter> {
    globals.bind(qh, 1..=1, ViewporterData).ok()
}

pub fn get(viewporter: &WpViewporter, surface: &WlSurface, qh: &QueueHandle<App>) -> WpViewport {
    viewporter.get_viewport(surface, qh, ViewportData)
}
