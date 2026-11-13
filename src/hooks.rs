//! Optional host callback invoked when a schema widget changes.
//!
//! The stock `settings` binary never registers a callback. A host that links
//! this crate can register one before [`crate::run`] and receive the record
//! that changed, then return an image to show beside that record.

use std::sync::Mutex;

/// Values of one record at the moment a field inside it changed.
pub struct Change {
    /// Callback name declared on the widget (`on_change` in the schema).
    pub callback: String,
    /// Path of the record, such as `pets.bearded_dragons[0]`.
    pub record: String,
    /// Field key and its current value, in schema order.
    pub values: Vec<(String, String)>,
}

/// Image returned by the host. Pixels are straight RGBA, top to bottom.
pub struct Preview {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

type Hook = Box<dyn FnMut(&Change) -> Option<Preview> + Send>;

static HOOK: Mutex<Option<Hook>> = Mutex::new(None);

/// Register the function Settings calls after a value change.
///
/// Replaces any previously registered function. The stock binary does not call this.
pub fn set_on_change(hook: impl FnMut(&Change) -> Option<Preview> + Send + 'static) {
    *HOOK.lock().expect("on_change hook") = Some(Box::new(hook));
}

pub(crate) fn on_change(change: &Change) -> Option<Preview> {
    let mut guard = HOOK.lock().expect("on_change hook");
    let hook = guard.as_mut()?;
    hook(change)
}
