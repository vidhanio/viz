//! GPU compositor hook driving `rune-fx` shimmer effects on
//! [`rune::El::effect`] slots while keeping idle windows at zero frames.
//!
//! Two shimmer modes share one compositor pass:
//! - Shape shimmer ([`SHIMMER_MARK`]): sweeps a soft translucent sheen across a
//!   rounded skeleton bar (`use_mask: false`).
//! - Glyph-masked shimmer ([`TEXT_SHIMMER_START`] / [`TEXT_SHIMMER_END`]):
//!   renders only the enclosed icon and text instances into a reusable mask
//!   texture and sweeps the sheen strictly across those glyph pixels
//!   (`use_mask: true`), leaving the background untouched.
//!
//! Shape shimmer can draw inside Rune's isolated layers, whose `Rgba16Float`
//! target uses a different pipeline from the frame target.

use std::{collections::HashMap, ops::Range};

use rune::{
    compose::{Compositor, DeviceKey, Frame},
    render::Kind,
};
use rune_fx::{
    Fx, FxFrame, FxLayer, FxOptions, FxRenderer, FxSlot, Place, Rect, Rgb, Shape, ShimmerParams,
    Source,
};

use super::tokens::SHIMMER_PERIOD;
use crate::app::Desktop;

/// Effect slot tag identifying rounded-rect skeleton shimmer bands.
pub(super) const SHIMMER_MARK: u32 = 1;
/// Effect slot tag opening a glyph-masked text shimmer range.
pub(super) const TEXT_SHIMMER_START: u32 = 2;
/// Effect slot tag closing a glyph-masked text shimmer range and drawing the
/// sheen over the enclosed glyphs.
pub(super) const TEXT_SHIMMER_END: u32 = 3;

struct MaskTarget {
    device: DeviceKey,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl MaskTarget {
    fn new(
        device_key: DeviceKey,
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viz-shimmer-mask"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            device: device_key,
            size,
            format,
            _texture: texture,
            view,
        }
    }

    fn matches(
        &self,
        device_key: DeviceKey,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> bool {
        self.device == device_key && self.size == size && self.format == format
    }
}

/// Compositor drawing `rune-fx` shimmer layers at [`SHIMMER_MARK`] and
/// [`TEXT_SHIMMER_END`] effect slots in painter order.
///
/// When no shimmer slot is present on screen, the compositor delegates straight
/// to [`Frame::draw_ui`] and reports `animating() == false` and
/// `draws_own() == false`, so idle windows draw zero frames.
#[derive(Default)]
pub struct FxCompositor {
    renderer: Option<(DeviceKey, wgpu::TextureFormat, FxRenderer)>,
    mask: Option<MaskTarget>,
    layers: Vec<FxLayer>,
    slots: Vec<FxSlot>,
    indices: HashMap<u32, usize>,
    mask_ranges: Vec<Range<u32>>,
    has_shimmer: bool,
    animating: bool,
}

impl std::fmt::Debug for FxCompositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxCompositor")
            .field("has_shimmer", &self.has_shimmer)
            .field("animating", &self.animating)
            .finish_non_exhaustive()
    }
}

impl FxCompositor {
    fn collect_shimmer(&mut self, frame: &Frame<'_, Desktop>) -> f32 {
        self.layers.clear();
        self.slots.clear();
        self.indices.clear();
        self.mask_ranges.clear();

        #[expect(
            clippy::cast_possible_truncation,
            reason = "wall-clock seconds for periodic shader phase fit in f32"
        )]
        let time = frame.rt.t as f32;
        let reduced = frame.rt.reduced_motion;
        let mut frame_scale = 1.0_f32;
        let mut layer_depth = 0_u32;
        let mut open_text_start: Option<u32> = None;

        for (idx, inst) in frame.rt.instances.iter().enumerate() {
            match inst.kind() {
                Some(Kind::Layer) => {
                    layer_depth = layer_depth.saturating_add(1);
                    open_text_start = None;
                    continue;
                }
                Some(Kind::LayerEnd) => {
                    layer_depth = layer_depth.saturating_sub(1);
                    open_text_start = None;
                    continue;
                }
                Some(Kind::Effect) => {}
                _ => continue,
            }
            let Ok(slot_idx) = u32::try_from(idx) else {
                continue;
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "effect slot mark is stored as two exact 16-bit integer floats"
            )]
            let mark = (inst.border[0] as u32) | ((inst.border[1] as u32) << 16);
            match mark {
                SHIMMER_MARK => {
                    let scale = inst.params[3].max(1.0);
                    frame_scale = scale;
                    self.push_shape_shimmer(slot_idx, inst, scale, time, reduced);
                }
                TEXT_SHIMMER_START => {
                    open_text_start = (layer_depth == 0).then_some(slot_idx);
                }
                TEXT_SHIMMER_END => {
                    let start = open_text_start.take();
                    if layer_depth != 0 {
                        continue;
                    }
                    let Some(start_idx) = start else {
                        continue;
                    };
                    let glyph_start = start_idx.saturating_add(1);
                    if glyph_start >= slot_idx {
                        continue;
                    }
                    let scale = inst.params[3].max(1.0);
                    frame_scale = scale;
                    self.mask_ranges.push(glyph_start..slot_idx);
                    self.push_text_shimmer(slot_idx, inst, frame.size, scale, time, reduced);
                }
                _ => {}
            }
        }
        frame_scale
    }

    fn push_shape_shimmer(
        &mut self,
        slot_idx: u32,
        inst: &rune::render::Instance,
        scale: f32,
        time: f32,
        reduced: bool,
    ) {
        let [left, top, width, height] = inst.rect;
        let [red, green, blue, radius] = inst.uv;
        let layer_index = self.layers.len();
        self.layers.push(FxLayer {
            fx: Fx::shimmer(ShimmerParams {
                color: Rgb([red, green, blue]),
                period: SHIMMER_PERIOD,
                band: 52.0,
                angle: 0.28,
                intensity: 0.24,
                pause: 0.2,
                ..ShimmerParams::default()
            }),
            shape: Shape::rounded(
                left / scale,
                top / scale,
                width / scale,
                height / scale,
                radius,
            ),
            place: Place::Over,
            time,
            seed: u32::try_from(layer_index).unwrap_or(0),
            reduced_motion: reduced,
            source: None,
            opacity: 1.0,
        });
        self.slots
            .push(FxSlot::default().scale(scale).clip(inst.clip, inst.fade));
        self.indices.insert(slot_idx, layer_index);
    }

    fn push_text_shimmer(
        &mut self,
        slot_idx: u32,
        inst: &rune::render::Instance,
        viewport: (u32, u32),
        scale: f32,
        time: f32,
        reduced: bool,
    ) {
        let [left, top, width, height] = inst.rect;
        let [red, green, blue, intensity] = inst.uv;
        let layer_index = self.layers.len();
        #[expect(
            clippy::cast_precision_loss,
            reason = "viewport pixel dimensions fit cleanly in f32 points"
        )]
        let source_rect = Rect::new(
            0.0,
            0.0,
            viewport.0 as f32 / scale,
            viewport.1 as f32 / scale,
        );
        self.layers.push(FxLayer {
            fx: Fx::shimmer(ShimmerParams {
                color: Rgb([red, green, blue]),
                period: SHIMMER_PERIOD,
                band: 30.0,
                angle: 0.25,
                intensity: if intensity > 0.0 { intensity } else { 0.85 },
                pause: 0.25,
                use_mask: true,
                ..ShimmerParams::default()
            }),
            shape: Shape::rounded(
                left / scale,
                top / scale,
                width / scale,
                height / scale,
                0.0,
            ),
            place: Place::Over,
            time,
            seed: u32::try_from(layer_index).unwrap_or(0),
            reduced_motion: reduced,
            source: Some(Source {
                slot: 0,
                rect: source_rect,
            }),
            opacity: 1.0,
        });
        self.slots
            .push(FxSlot::default().scale(scale).clip(inst.clip, inst.fade));
        self.indices.insert(slot_idx, layer_index);
    }
}

impl Compositor<Desktop> for FxCompositor {
    fn render(&mut self, frame: &mut Frame<'_, Desktop>) {
        let frame_scale = self.collect_shimmer(frame);
        if self.layers.is_empty() {
            self.has_shimmer = false;
            self.animating = false;
            frame.draw_ui();
            return;
        }

        self.has_shimmer = true;
        self.animating = !frame.rt.reduced_motion;

        let device_key = frame.device_key();
        let needs_new = self
            .renderer
            .as_ref()
            .is_none_or(|(key, format, _)| *key != device_key || *format != frame.format);
        if needs_new {
            let mut renderer = FxRenderer::new(frame.device, frame.format, FxOptions::default());
            renderer.add_format(frame.device, wgpu::TextureFormat::Rgba16Float);
            self.renderer = Some((device_key, frame.format, renderer));
        }
        let clear = frame.rt.clear_color();
        frame.ui.set_fade_stops(frame.rt.fade_stops());
        frame.ui.prepare(
            frame.device,
            frame.queue,
            &mut frame.rt.text.atlas,
            &frame.rt.instances,
            frame.size,
        );

        let has_mask = !self.mask_ranges.is_empty();
        if has_mask {
            let recreate = self
                .mask
                .as_ref()
                .is_none_or(|mask| !mask.matches(device_key, frame.size, frame.format));
            if recreate {
                self.mask = Some(MaskTarget::new(
                    device_key,
                    frame.device,
                    frame.size,
                    frame.format,
                ));
            }
            if let Some(mask) = self.mask.as_ref() {
                let mut pass = frame
                    .encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("viz-shimmer-mask-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &mask.view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                for range in &self.mask_ranges {
                    frame.ui.draw_range(&mut pass, frame.format, range.clone());
                }
            }
        }

        let Some((_, _, fx)) = self.renderer.as_mut() else {
            frame.draw_ui();
            return;
        };
        let mask_view = has_mask
            .then(|| self.mask.as_ref().map(|mask| &mask.view))
            .flatten();
        let sources: Vec<&wgpu::TextureView> = mask_view.into_iter().collect();
        fx.set_srgb_blend(frame.srgb_blend());
        fx.set_fade_stops(frame.fade_stops());
        fx.prepare_with_slots(
            frame.device,
            frame.queue,
            &FxFrame {
                viewport: frame.size.into(),
                scale: frame_scale,
                layers: &self.layers,
                backdrop: None,
                sources: &sources,
            },
            &self.slots,
        );

        let indices = &self.indices;
        frame
            .ui
            .draw_with(frame.encoder, frame.target, clear, &mut |pass, site| {
                if let Some(&layer) = indices.get(&site.index) {
                    fx.draw_layer_in(pass, site.format, site.origin, layer);
                }
            });
    }

    fn animating(&self) -> bool {
        self.animating
    }

    fn draws_own(&self) -> bool {
        self.has_shimmer
    }
}
