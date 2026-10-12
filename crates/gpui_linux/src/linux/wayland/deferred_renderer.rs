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
//! The slowest part of creating the first renderer, the wgpu instance (about 28 ms, loading every
//! Vulkan and EGL driver), needs only the display, so [`prewarm_instance`] starts it on a thread of
//! its own when the client connects. The first window builds its GPU context from that instance,
//! and every later window shares the context.
//!
//! Zetta-authored, with no upstream counterpart; `window.rs` holds one per window.

use std::{
    borrow::Cow,
    cell::{OnceCell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    sync::Arc,
    thread::JoinHandle,
};

use gpui::{AtlasKey, AtlasTile, DevicePixels, PlatformAtlas, Size};
use gpui_wgpu::{
    CompositorGpuHint, GpuContext, WgpuContext, WgpuRenderer, WgpuSurfaceConfig, wgpu,
};
use raw_window_handle as rwh;

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
    /// The instance [`prewarm_instance`] started, for the first window only.
    instance: Option<JoinHandle<wgpu::Instance>>,
}

impl DeferredRenderer {
    pub(super) fn new(
        gpu_context: GpuContext,
        window: RawWindow,
        config: WgpuSurfaceConfig,
        compositor_gpu: Option<CompositorGpuHint>,
        instance: Option<JoinHandle<wgpu::Instance>>,
    ) -> Self {
        Self {
            pending: Some(Box::new(PendingRenderer {
                gpu_context,
                window,
                config,
                compositor_gpu,
                subpixel_bgr: None,
                instance,
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
            instance,
        } = *pending;
        if let Some(instance) = instance {
            create_context_from(instance, &gpu_context, &window, compositor_gpu);
        }
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

/// Starts creating the wgpu instance for the display behind `display`, a `wl_display`.
pub(super) fn prewarm_instance(display: *mut c_void) -> Option<JoinHandle<wgpu::Instance>> {
    let display = RawDisplay(NonNull::new(display)?);
    std::thread::Builder::new()
        .name("wgpu instance".to_owned())
        .spawn(move || WgpuContext::instance(Box::new(display)))
        .map_err(|error| log::warn!("could not start creating the wgpu instance early: {error}"))
        .ok()
}

/// Fills the shared GPU context from a prewarmed instance, the way `WgpuRenderer::new` would
/// from one it created itself: the adapter is chosen by configuring a surface of this window on
/// it. That surface is dropped before the renderer creates its own. Anything that goes wrong
/// leaves the context empty, and the renderer then creates its instance as it always did.
fn create_context_from(
    instance: JoinHandle<wgpu::Instance>,
    gpu_context: &GpuContext,
    window: &RawWindow,
    compositor_gpu: Option<CompositorGpuHint>,
) {
    let Ok(instance) = instance.join() else {
        log::error!("creating the wgpu instance early panicked");
        return;
    };
    if gpu_context.borrow().is_some() {
        return;
    }
    let context = (|| {
        let handle = rwh::HasWindowHandle::window_handle(window)?;
        // SAFETY: the surface is dropped at the end of this closure, while the window it was
        // created for is still open.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: None,
                raw_window_handle: handle.as_raw(),
            })
        }?;
        WgpuContext::new(instance, &surface, compositor_gpu)
    })();
    match context {
        Ok(context) => *gpu_context.borrow_mut() = Some(context),
        Err(error) => {
            log::warn!("could not use the early wgpu instance, creating another: {error:#}")
        }
    }
}

/// A `wl_display`, which outlives every thread that borrows it.
#[derive(Debug)]
struct RawDisplay(NonNull<c_void>);

// SAFETY: libwayland's display is thread-safe, and the connection is open for the life of the
// process; the instance only reads the pointer.
unsafe impl Send for RawDisplay {}
unsafe impl Sync for RawDisplay {}

impl rwh::HasDisplayHandle for RawDisplay {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let handle = rwh::WaylandDisplayHandle::new(self.0);
        // SAFETY: see above.
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
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
