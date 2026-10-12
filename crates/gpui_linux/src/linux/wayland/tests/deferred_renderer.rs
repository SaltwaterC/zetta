use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{AtlasTextureId, AtlasTextureKind, Bounds, Point, RenderSvgParams, TileId};

use super::*;

/// Records what reached it, and answers every lookup with the same tile.
#[derive(Default)]
struct RecordingAtlas {
    inserted: RefCell<Vec<AtlasKey>>,
    removed: RefCell<Vec<AtlasKey>>,
}

impl PlatformAtlas for RecordingAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> anyhow::Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> anyhow::Result<Option<AtlasTile>> {
        build()?;
        self.inserted.borrow_mut().push(key.clone());
        Ok(Some(tile()))
    }

    fn remove(&self, key: &AtlasKey) {
        self.removed.borrow_mut().push(key.clone());
    }
}

fn tile() -> AtlasTile {
    AtlasTile {
        texture_id: AtlasTextureId {
            index: 3,
            kind: AtlasTextureKind::Monochrome,
        },
        tile_id: TileId(7),
        padding: 0,
        bounds: Bounds {
            origin: Point::default(),
            size: Size {
                width: DevicePixels(2),
                height: DevicePixels(2),
            },
        },
    }
}

fn key() -> AtlasKey {
    AtlasKey::Svg(RenderSvgParams {
        path: "icons/test.svg".into(),
        size: Size {
            width: DevicePixels(2),
            height: DevicePixels(2),
        },
    })
}

fn build_counting(calls: &RefCell<usize>) -> impl FnMut() -> BuildResult + '_ {
    move || {
        *calls.borrow_mut() += 1;
        Ok(Some((
            Size {
                width: DevicePixels(2),
                height: DevicePixels(2),
            },
            Cow::Owned(vec![0; 4]),
        )))
    }
}

type BuildResult = anyhow::Result<Option<(Size<DevicePixels>, Cow<'static, [u8]>)>>;

#[test]
fn painting_into_an_unbound_atlas_binds_it_first() {
    let atlas = Arc::new(DeferredAtlas::default());
    let renderers = Arc::new(RecordingAtlas::default());
    let binds = Rc::new(Cell::new(0));
    *atlas.create.borrow_mut() = Some(Box::new({
        let atlas = Arc::downgrade(&atlas);
        let renderers = renderers.clone();
        let binds = binds.clone();
        move || {
            binds.set(binds.get() + 1);
            atlas.upgrade().unwrap().bind(renderers.clone());
            Ok(())
        }
    }));
    let calls = RefCell::new(0);

    let first = atlas
        .get_or_insert_with(&key(), &mut build_counting(&calls))
        .unwrap();
    let second = atlas
        .get_or_insert_with(&key(), &mut build_counting(&calls))
        .unwrap();

    assert_eq!(first, Some(tile()));
    assert_eq!(second, Some(tile()));
    assert_eq!(binds.get(), 1);
    assert_eq!(*calls.borrow(), 2);
}

#[test]
fn painting_fails_when_no_renderer_can_be_created() {
    let atlas = DeferredAtlas::default();
    *atlas.create.borrow_mut() = Some(Box::new(|| anyhow::bail!("no GPU")));
    let calls = RefCell::new(0);

    let painted = atlas.get_or_insert_with(&key(), &mut build_counting(&calls));
    atlas.remove(&key());

    assert!(painted.is_err());
    assert_eq!(*calls.borrow(), 0);
}

#[test]
fn a_bound_atlas_forwards_to_the_renderers() {
    let atlas = DeferredAtlas::default();
    let renderers = Arc::new(RecordingAtlas::default());
    atlas.bind(renderers.clone());
    let calls = RefCell::new(0);

    let tile = atlas
        .get_or_insert_with(&key(), &mut build_counting(&calls))
        .unwrap();
    atlas.remove(&key());

    assert_eq!(tile, Some(self::tile()));
    assert_eq!(*calls.borrow(), 1);
    // `AtlasKey` is not `Debug`.
    assert!(*renderers.inserted.borrow() == vec![key()]);
    assert!(*renderers.removed.borrow() == vec![key()]);
}

#[test]
fn binding_again_keeps_the_first_atlas() {
    let atlas = DeferredAtlas::default();
    let first = Arc::new(RecordingAtlas::default());
    let second = Arc::new(RecordingAtlas::default());
    atlas.bind(first.clone());
    atlas.bind(second.clone());

    atlas.remove(&key());

    assert_eq!(first.removed.borrow().len(), 1);
    assert!(second.removed.borrow().is_empty());
}
