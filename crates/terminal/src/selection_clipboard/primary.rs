//! Linux/FreeBSD primary-selection publication and middle-click reads.

use super::*;

pub(crate) fn copy(term: &AlacrittyTerm, cx: &mut App) {
    request(1, term, App::write_to_primary, cx);
}

pub(crate) fn read(cx: &App) -> Task<Option<ClipboardItem>> {
    read_slot(1, App::read_from_primary_async, cx)
}

fn nonempty(item: &Option<ClipboardItem>) -> bool {
    item.as_ref()
        .and_then(|item| item.text())
        .is_some_and(|text| !text.is_empty())
}

pub(super) fn read_for_paste(cx: &App) -> Task<Option<ClipboardItem>> {
    match try_read(read(cx)) {
        Ok(item) if nonempty(&item) => Task::ready(item),
        Ok(_) => super::read(cx),
        Err(primary) => cx.spawn(async move |cx| {
            let item = primary.await;
            if nonempty(&item) {
                item
            } else {
                cx.update(|cx| super::read(cx)).await
            }
        }),
    }
}
