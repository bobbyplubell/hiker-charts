//! egui comfy builder and interactive preview for `hiker-charts`.
//!
//! A pure `egui` panel layer (no `eframe`/`winit`): the host owns the window and
//! event loop. It drives a `BuilderState` over a `ChartSpec` with column-populated
//! dropdowns, rasterizes the plotters SVG through resvg to an egui texture, and
//! offers a pan- and pinch-zoomable preview that reads gestures from egui's
//! `InputState` exactly as `hiker-canvas` does. See `IMPLEMENTATION.md` §6.

pub mod camera;
pub mod data_view;
pub mod model;
pub mod panel;
pub mod preview;
pub mod raster;
