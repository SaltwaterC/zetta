//! A window's renderer, created at the window's first frame rather than with the window.
//!
//! Creating a [`WgpuRenderer`] is nearly all of the cost of opening a window: the first one loads
//! every Vulkan and EGL driver on the system, picks an adapter by configuring the surface on it,
//! and builds the pipelines, about 60 ms on a laptop. Nothing draws until the compositor has
//! configured the surface, and the application builds the window's contents in between, so
//! creating the renderer when the window opens put all of that ahead of the application's own
//! start-up. Zetta starts its first terminal's child while building the window's contents; created
//! here instead, the GPU is set up while the child already runs and its output is parsed.
//!
//! GPUI takes the window's sprite atlas when it creates the window, before the application has
//! built its contents, so the window hands out a [`DeferredAtlas`] that forwards to the renderer's
//! own. The first tile painted into it creates the renderer: GPUI lays the window out and paints it
//! once as soon as the application has built its contents, before the compositor's first frame.
//!
//! Zetta-authored, with no upstream counterpart; `window.rs` holds one per window.

use std::{
    borrow::Cow,
    cell::{OnceCell, RefCell},
    sync::Arc,
};

use gpui::{AtlasKey, AtlasTile, DevicePixels, PlatformAtlas, Size};
use gpui_wgpu::{CompositorGpuHint, GpuContext, WgpuRenderer, WgpuSurfaceConfig};

use super::window::RawWindow;

/// A renderer that is created on first use, taking the window settings made before then with it.
pub(super) struct DeferredRenderer {
    pending: Option<Box<PendingRenderer>>,
    renderer: Option<WgpuRenderer>,
    atlas: Arc<DeferredAtlas>,
}

/// What creating the renderer takes, kept current until it is created.
struct PendingRenderer {
    gpu_context: GpuContext,
    window: RawWindow,
    /// Its size and transparency follow the window until the renderer is created.
    config: WgpuSurfaceConfig,
    compositor_gpu: Option<CompositorGpuHint>,
    subpixel_bgr: Option<bool>,
}

impl DeferredRenderer {
    pub(super) fn new(
        gpu_context: GpuContext,
        window: RawWindow,
        config: WgpuSurfaceConfig,
        compositor_gpu: Option<CompositorGpuHint>,
    ) -> Self {
        Self {
            pending: Some(Box::new(PendingRenderer {
                gpu_context,
                window,
                config,
                compositor_gpu,
                subpixel_bgr: None,
            })),
            renderer: None,
            atlas: Arc::default(),
        }
    }

    /// Creates the renderer unless it already exists. `Ok(true)` when this call created it.
    ///
    /// A failure leaves nothing to retry with, so every later call fails too; the window closes on
    /// it.
    pub(super) fn create(&mut self) -> anyhow::Result<bool> {
        let Some(pending) = self.pending.take() else {
            anyhow::ensure!(
                self.renderer.is_some(),
                "this window's GPU renderer could not be created"
            );
            return Ok(false);
        };
        let PendingRenderer {
            gpu_context,
            window,
            config,
            compositor_gpu,
            subpixel_bgr,
        } = *pending;
        let mut renderer = WgpuRenderer::new(gpu_context, &window, config, compositor_gpu)?;
        if let Some(is_bgr) = subpixel_bgr {
            renderer.set_subpixel_layout(is_bgr);
        }
        self.atlas.bind(renderer.sprite_atlas().clone());
        self.renderer = Some(renderer);
        Ok(true)
    }

    /// The renderer, if it has been created.
    pub(super) fn created(&mut self) -> Option<&mut WgpuRenderer> {
        self.renderer.as_mut()
    }

    pub(super) fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    /// How the atlas has its window create the renderer, when it is painted into first.
    pub(super) fn create_on_first_paint(&self, create: Box<dyn Fn() -> anyhow::Result<()>>) {
        *self.atlas.create.borrow_mut() = Some(create);
    }

    pub(super) fn set_subpixel_layout(&mut self, is_bgr: bool) {
        match (&mut self.renderer, &mut self.pending) {
            (Some(renderer), _) => renderer.set_subpixel_layout(is_bgr),
            (None, Some(pending)) => pending.subpixel_bgr = Some(is_bgr),
            (None, None) => {}
        }
    }

    pub(super) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        match (&mut self.renderer, &mut self.pending) {
            (Some(renderer), _) => renderer.update_drawable_size(size),
            (None, Some(pending)) => pending.config.size = size,
            (None, None) => {}
        }
    }

    pub(super) fn update_transparency(&mut self, transparent: bool) {
        match (&mut self.renderer, &mut self.pending) {
            (Some(renderer), _) => renderer.update_transparency(transparent),
            (None, Some(pending)) => pending.config.transparent = transparent,
            (None, None) => {}
        }
    }

    pub(super) fn destroy(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            renderer.destroy();
        }
    }
}

/// The window's sprite atlas: the renderer's own, once there is a renderer.
///
/// Tiles are only inserted by painting, and the first one creates the renderer, so an atlas that
/// is not yet bound has none to remove.
#[derive(Default)]
pub(super) struct DeferredAtlas {
    atlas: OnceCell<Arc<dyn PlatformAtlas>>,
    create: RefCell<Option<Box<dyn Fn() -> anyhow::Result<()>>>>,
}

impl DeferredAtlas {
    pub(super) fn bind(&self, atlas: Arc<dyn PlatformAtlas>) {
        if self.atlas.set(atlas).is_err() {
            log::error!("a window's sprite atlas was bound twice; keeping the first");
        }
    }
}

impl PlatformAtlas for DeferredAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> anyhow::Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> anyhow::Result<Option<AtlasTile>> {
        if self.atlas.get().is_none()
            && let Some(create) = self.create.borrow().as_ref()
        {
            create()?;
        }
        match self.atlas.get() {
            Some(atlas) => atlas.get_or_insert_with(key, build),
            None => anyhow::bail!("painted into a window that has no renderer"),
        }
    }

    fn remove(&self, key: &AtlasKey) {
        if let Some(atlas) = self.atlas.get() {
            atlas.remove(key);
        }
    }
}

#[cfg(test)]
#[path = "tests/deferred_renderer.rs"]
mod tests;
