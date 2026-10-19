//! Persistent framebuffer and accumulated damage for WGPU presentation.
use crate::wgpu_renderer::{RendererState, WgpuRenderer};
use gpui::{Bounds, DevicePixels, ScaledPixels, Scene, SceneDamage, Size};

/// A pixel-aligned scissor rectangle, clamped to the render target.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScissorRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl ScissorRect {
    /// Clamps a damage rectangle to the target, aligned outward to whole
    /// pixels. `None` when no on-target pixels are covered.
    fn from_bounds(
        rect: &Bounds<ScaledPixels>,
        target_width: u32,
        target_height: u32,
    ) -> Option<ScissorRect> {
        let left = (rect.origin.x.0.floor().max(0.0) as u32).min(target_width);
        let top = (rect.origin.y.0.floor().max(0.0) as u32).min(target_height);
        let right =
            ((rect.origin.x.0 + rect.size.width.0).ceil().max(0.0) as u32).min(target_width);
        let bottom =
            ((rect.origin.y.0 + rect.size.height.0).ceil().max(0.0) as u32).min(target_height);
        if right <= left || bottom <= top {
            return None;
        }
        Some(ScissorRect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }

    fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    fn union(&self, other: &ScissorRect) -> ScissorRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        ScissorRect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    fn covers(&self, target_width: u32, target_height: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width >= target_width && self.height >= target_height
    }
}

/// The regions a frame should re-render: everything, nothing, or a set of
/// pairwise-disjoint scissor rectangles that each get their own set of draws.
enum RenderRegions {
    Full,
    None,
    Partial(smallvec::SmallVec<[ScissorRect; 4]>),
}

/// Plans scissor regions for the given damage. The damage rectangles are
/// pairwise disjoint (guaranteed by `DamageRects`), so drawing each region
/// separately never double-blends a pixel. When the regions nearly fill
/// their bounding rectangle the plan collapses to that rectangle: repeated
/// draws only pay off when they skip substantial area between the regions.
fn plan_render_regions(
    rects: &[Bounds<ScaledPixels>],
    target_width: u32,
    target_height: u32,
) -> RenderRegions {
    let mut regions: smallvec::SmallVec<[ScissorRect; 4]> = rects
        .iter()
        .filter_map(|rect| ScissorRect::from_bounds(rect, target_width, target_height))
        .collect();
    let mut merged: smallvec::SmallVec<[ScissorRect; 4]> = smallvec::SmallVec::new();
    for mut region in regions.drain(..) {
        while let Some(index) = merged.iter().position(|other| {
            region.x < other.x + other.width
                && region.x + region.width > other.x
                && region.y < other.y + other.height
                && region.y + region.height > other.y
        }) {
            region = region.union(&merged.swap_remove(index));
        }
        merged.push(region);
    }
    regions = merged;
    let Some(bounding) = regions
        .iter()
        .copied()
        .reduce(|bounding, region| bounding.union(&region))
    else {
        return RenderRegions::None;
    };
    let covered: u64 = regions.iter().map(ScissorRect::area).sum();
    if covered * 10 >= bounding.area() * 7 {
        if bounding.covers(target_width, target_height) {
            return RenderRegions::Full;
        }
        regions.clear();
        regions.push(bounding);
    }
    RenderRegions::Partial(regions)
}

/// The persistent texture partial rendering draws into. Unlike swapchain
/// images (which rotate), its contents survive across frames, so a frame can
/// re-render only its damaged region and copy the whole texture to the
/// swapchain.
struct FrameTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Whether the texture's contents match the most recently recorded
    /// scene. False after (re)creation and GPU errors, forcing a full
    /// render.
    valid: bool,
}

pub(crate) struct RendererDamage {
    texture: Option<FrameTexture>,
    pending: SceneDamage,
}
impl Default for RendererDamage {
    fn default() -> Self {
        Self {
            texture: None,
            pending: SceneDamage::Full,
        }
    }
}
impl RendererDamage {
    pub(crate) fn invalidate(&mut self) {
        if let Some(texture) = self.texture.take() {
            texture.texture.destroy();
        }
        self.pending = SceneDamage::Full;
    }
}
pub(crate) fn surface_usage(caps: &wgpu::SurfaceCapabilities) -> wgpu::TextureUsages {
    wgpu::TextureUsages::RENDER_ATTACHMENT | (caps.usages & wgpu::TextureUsages::COPY_DST)
}
impl WgpuRenderer {
    /// Redraws damaged regions, preserving all pending changes until presentation succeeds.
    pub fn draw_with_damage(&mut self, scene: &Scene, damage: &SceneDamage) -> bool {
        draw(self, scene, Some(damage))
    }
}

pub(crate) fn draw(
    renderer: &mut WgpuRenderer,
    scene: &Scene,
    damage: Option<&SceneDamage>,
) -> bool {
    let damage = damage.filter(|_| {
        renderer
            .surface_config
            .usage
            .contains(wgpu::TextureUsages::COPY_DST)
    });

    // Failed and skipped frames must not lose their damage, so it is
    // accumulated up front and only cleared after a successful present.
    if let Some(damage) = damage {
        let pending = std::mem::replace(&mut renderer.fast_damage.pending, SceneDamage::Unchanged);
        renderer.fast_damage.pending = pending.union(damage.clone());
    }

    #[cfg(target_family = "wasm")]
    if renderer.device_lost() {
        if matches!(renderer.state, RendererState::Ready { .. }) {
            log::error!(
                "Browser graphics context was lost; rendering has stopped. Reload the page to recover."
            );
            renderer.unconfigure_surface();
        }
        return false;
    }

    // Bail out early if the surface has been unconfigured (e.g. during
    // Android background/rotation transitions).  Attempting to acquire
    // a texture from an unconfigured surface can block indefinitely on
    // some drivers (Adreno).
    let RendererState::Ready { surface, core } = &mut renderer.state else {
        return false;
    };

    if let Some(error) = renderer.last_surface_error.take().or_else(|| {
        renderer
            .device_errors
            .observe_error(&mut renderer.observed_error_generation)
    }) {
        renderer.fast_damage.invalidate();
        renderer.failed_frame_count += 1;
        log::error!(
            "GPU error during frame (failure {} of 10): {error}",
            renderer.failed_frame_count
        );

        // TBD. Does retrying more actually help?
        if renderer.failed_frame_count > 10 {
            panic!("Too many consecutive GPU errors. Last error: {error}");
        } else if renderer.failed_frame_count > 5 {
            core.resources.invalidate_intermediate_textures();
            renderer.atlas.clear();
            crate::fast::layers::TileCache::clear(&mut core.fast_frame.layers);
            renderer.needs_redraw = true;
            renderer.failed_frame_count = 0;
            return false;
        }
    } else {
        renderer.failed_frame_count = 0;
    }

    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame) => frame,
        wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
            // Textures must be destroyed before the surface can be reconfigured.
            drop(frame);
            surface.configure(&core.resources.device, &renderer.surface_config);
            return false;
        }
        wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
            surface.configure(&core.resources.device, &renderer.surface_config);
            return false;
        }
        wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
            return false;
        }
        wgpu::CurrentSurfaceTexture::Validation => {
            renderer.last_surface_error = Some("Surface texture validation error".to_string());
            return false;
        }
    };

    // The acquired texture is the authority on frame dimensions; the surface
    // configuration is only a request.
    let size = Size {
        width: DevicePixels(frame.texture.width() as i32),
        height: DevicePixels(frame.texture.height() as i32),
    };
    let premultiplied_alpha =
        renderer.surface_config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied;
    if damage.is_none() {
        // A direct render does not update the persistent texture.
        if let Some(frame_texture) = &mut renderer.fast_damage.texture {
            frame_texture.valid = false;
        }
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        if let Err(error) = core.render_frame(
            scene,
            &frame_view,
            size,
            premultiplied_alpha,
            wgpu::Color::TRANSPARENT,
        ) {
            core.fast_frame.damage_regions.clear();
            log::error!("{error:#}");
            return false;
        }
        core.resources.queue.present(frame);
        return true;
    }

    // Match the acquired texture rather than the requested surface size.
    let width = frame.texture.width();
    let height = frame.texture.height();
    if !renderer
        .fast_damage
        .texture
        .as_ref()
        .is_some_and(|texture| {
            texture.texture.width() == width && texture.texture.height() == height
        })
    {
        if let Some(texture) = renderer.fast_damage.texture.take() {
            texture.texture.destroy();
        }
        let texture = core
            .resources
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("persistent_frame_texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: renderer.surface_config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.fast_damage.texture = Some(FrameTexture {
            texture,
            view,
            valid: false,
        });
    }
    let frame_texture = renderer
        .fast_damage
        .texture
        .as_mut()
        .expect("frame texture was created");
    let render_regions = if frame_texture.valid {
        match &renderer.fast_damage.pending {
            SceneDamage::Unchanged => RenderRegions::None,
            SceneDamage::Full => RenderRegions::Full,
            SceneDamage::Rects(rects) => plan_render_regions(rects.as_slice(), width, height),
        }
    } else {
        RenderRegions::Full
    };
    let regions: Option<&[ScissorRect]> = match &render_regions {
        RenderRegions::None => None,
        RenderRegions::Full => Some(&[]),
        RenderRegions::Partial(regions) => Some(regions),
    };
    if let Some(regions) = regions {
        frame_texture.valid = false;
        core.fast_frame.damage_regions = regions.to_vec();
        if let Err(error) = core.render_frame(
            scene,
            &frame_texture.view,
            size,
            premultiplied_alpha,
            wgpu::Color::TRANSPARENT,
        ) {
            frame_texture.valid = false;
            core.fast_frame.damage_regions.clear();
            log::error!("{error:#}");
            return false;
        }
        core.fast_frame.damage_regions.clear();
        frame_texture.valid = true;
    }
    let mut encoder =
        core.resources
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("present_copy_encoder"),
            });
    encoder.copy_texture_to_texture(
        frame_texture.texture.as_image_copy(),
        frame.texture.as_image_copy(),
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    core.resources
        .queue
        .submit(std::iter::once(encoder.finish()));
    core.resources.queue.present(frame);
    renderer.fast_damage.pending = SceneDamage::Unchanged;
    true
}

pub(crate) fn clear_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("damage_clear"),
        source: wgpu::ShaderSource::Wgsl(include_str!("damage.wgsl").into()),
    });
    let clear_region_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("clear_region_layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let clear_region = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("clear_region"),
        layout: Some(&clear_region_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_clear_region"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_clear_region"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // No blending: overwrite the region with transparent
                // black, exactly like LoadOp::Clear would.
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    clear_region
}

pub(crate) fn draw_regions(
    pass: &mut wgpu::RenderPass<'_>,
    regions: &[ScissorRect],
    vertices: std::ops::Range<u32>,
    instances: std::ops::Range<u32>,
) {
    if regions.is_empty() {
        pass.draw(vertices, instances);
    } else {
        for region in regions {
            pass.set_scissor_rect(region.x, region.y, region.width, region.height);
            pass.draw(vertices.clone(), instances.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RenderRegions, plan_render_regions};
    use crate::wgpu_renderer::WgpuHeadlessRenderer;
    use gpui::{
        Bounds, DevicePixels, Hsla, PlatformHeadlessRenderer, ScaledPixels, Scene, SceneDamage,
        point, size,
    };

    fn scene(x: f32, layer: bool) -> Scene {
        let mut scene = Scene::default();
        for (x, y, w, h, color) in [
            (0., 0., 64., 32., gpui::black()),
            (x, 4.25, 8., 8., gpui::red()),
            (40., 4., 8., 8., gpui::blue()),
            (
                0.,
                0.,
                64.,
                32.,
                Hsla {
                    h: 0.33,
                    s: 1.,
                    l: 0.5,
                    a: 0.25,
                },
            ),
        ] {
            let bounds = Bounds::new(
                point(ScaledPixels(x), ScaledPixels(y)),
                size(ScaledPixels(w), ScaledPixels(h)),
            );
            scene.insert_primitive(gpui::Quad {
                bounds,
                content_mask: gpui::ContentMask { bounds },
                background: color.into(),
                ..Default::default()
            });
        }
        let mut path = gpui::Path::new(gpui::point(gpui::px(x), gpui::px(18.)));
        path.line_to(gpui::point(gpui::px(x + 9.), gpui::px(25.)));
        path.line_to(gpui::point(gpui::px(x), gpui::px(29.)));
        path.line_to(gpui::point(gpui::px(x), gpui::px(18.)));
        path.color = gpui::green().into();
        scene.insert_primitive(path.scale(1.));
        scene.finish();
        if !layer {
            return scene;
        }
        let bounds = Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(64.), ScaledPixels(32.)),
        );
        // Fast's analytic rounded clip wire format, including vertical-radii metadata.
        scene.layers.frames.push(gpui::LayerFrame {
            key: gpui::LayerKey(u32::MAX),
            generation: 0,
            background: gpui::Rgba {
                r: 8.,
                g: 8.,
                b: 8.,
                a: 8.,
            },
            tile_size: 0,
            content: Scene::default().into(),
            dirty_tiles: Vec::new(),
        });
        let mut root = Scene::default();
        root.layers.frames.push(gpui::LayerFrame {
            key: gpui::LayerKey(0x0100_0000),
            generation: 0,
            background: gpui::Rgba::default(),
            tile_size: 0,
            content: scene.into(),
            dirty_tiles: Vec::new(),
        });
        root.insert_primitive(gpui::PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale: false.into(),
            opacity: 1.,
            bounds,
            content_mask: gpui::ContentMask { bounds },
            corner_radii: gpui::Corners::all(ScaledPixels(8.)),
            tile: gpui::AtlasTile {
                texture_id: gpui::AtlasTextureId {
                    index: 0xF100_0000,
                    kind: gpui::AtlasTextureKind::Polychrome,
                },
                tile_id: gpui::TileId(0),
                padding: 0,
                bounds: Default::default(),
            },
        });
        root.finish();
        root
    }

    #[test]
    fn test_partial_matches_full() -> anyhow::Result<()> {
        let mut renderer = WgpuHeadlessRenderer::new()?;
        let size = size(DevicePixels(64), DevicePixels(32));
        for layer in [false, true] {
            let before = scene(4.25, layer);
            let after = scene(18.25, layer);
            let expected = renderer.render_scene_to_image(&after, size)?;
            renderer.render_scene(&before, size)?;
            let SceneDamage::Rects(rects) = SceneDamage::between(&before, &after) else {
                panic!("expected partial damage");
            };
            let RenderRegions::Partial(regions) = plan_render_regions(rects.as_slice(), 64, 32)
            else {
                panic!("expected partial render");
            };
            renderer.core.fast_frame.damage_regions = regions.into_vec();
            let view = renderer.render_target.as_ref().unwrap().view.clone();
            renderer
                .core
                .render_frame(&after, &view, size, false, wgpu::Color::TRANSPARENT)?;
            renderer.core.fast_frame.damage_regions.clear();
            assert_eq!(renderer.read_image()?, expected);
        }
        Ok(())
    }

    #[test]
    fn test_rounded_damage_is_disjoint() {
        let rects =
            [(0.1, 0.1, 1., 1.), (1.2, 0.1, 1., 1.), (20., 20., 1., 1.)].map(|(x, y, w, h)| {
                Bounds::new(
                    point(ScaledPixels(x), ScaledPixels(y)),
                    size(ScaledPixels(w), ScaledPixels(h)),
                )
            });
        let RenderRegions::Partial(regions) = plan_render_regions(&rects, 64, 32) else {
            panic!("expected partial render");
        };
        for (i, a) in regions.iter().enumerate() {
            for b in &regions[i + 1..] {
                assert!(
                    a.x + a.width <= b.x
                        || b.x + b.width <= a.x
                        || a.y + a.height <= b.y
                        || b.y + b.height <= a.y
                );
            }
        }
    }
}
