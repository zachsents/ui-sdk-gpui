//! Port of `foundation/lists/listbox-listing.ts`: the listing, what each
//! option of a list shows from the list as a whole, by its key. Whether the
//! query hides an option and whether it's selected depend on state every
//! option shares, like the query and the value; an option reads its own
//! entry instead, which changes only when what it shows does.
//!
//! The original keeps the listing in the component's state so a change
//! reaches only the options whose entry changed. gpui renders a component's
//! parts together, so the listing is worked out as parts read it, again only
//! when the list or the selection's inputs changed, never for a highlight
//! move.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use gpui::SharedString;

use crate::listbox_engine::{ListboxEngine, ListingInputs};

/// Each option's entry, by key, from [`ListboxEngine::listing`]
/// (`Listing`). The same `Rc` while no entry changed.
pub type Listing = Rc<HashMap<SharedString, SharedString>>;

thread_local! {
    static NO_LISTING: Listing = Rc::new(HashMap::new());
}

/// The listing before a list has worked one out (`noListing`).
pub fn no_listing() -> Listing {
    NO_LISTING.with(Rc::clone)
}

/// What the selection adds to the listing: what an option adds to its entry,
/// like `"s"` while it's selected, by its key and value (`None` for a
/// built-in option), and the state that decides it, like the value.
pub struct ListingSelection<'a, T, I> {
    pub entry: &'a dyn Fn(&SharedString, Option<&T>) -> SharedString,
    pub inputs: I,
}

struct Last<T, X, I> {
    inputs: ListingInputs<T, X>,
    selection: Option<I>,
    listing: Listing,
}

/// Keeps a listing current (`keepListing`): it looks again only when the
/// list or the selection's inputs changed. Make it with [`keep_listing`] and
/// keep it in a field.
pub struct KeepListing<T, X = (), I = ()> {
    last: RefCell<Option<Last<T, X, I>>>,
}

impl<T, X, I> Default for KeepListing<T, X, I> {
    fn default() -> Self {
        Self {
            last: RefCell::new(None),
        }
    }
}

/// A [`KeepListing`] with nothing worked out yet.
pub fn keep_listing<T, X, I>() -> KeepListing<T, X, I> {
    KeepListing::default()
}

impl<T, X, I> KeepListing<T, X, I>
where
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
    I: PartialEq,
{
    /// The listing now, for `listbox` reading `this`, with what `selection`
    /// adds to each entry.
    pub fn listing<C: 'static>(
        &self,
        this: &C,
        listbox: &ListboxEngine<C, T, X>,
        selection: Option<ListingSelection<'_, T, I>>,
    ) -> Listing {
        let inputs = listbox.listing_inputs(this);
        let (entry, selection_inputs) = match selection {
            Some(selection) => (Some(selection.entry), Some(selection.inputs)),
            None => (None, None),
        };
        if let Some(last) = self.last.borrow().as_ref()
            && last.inputs == inputs
            && last.selection == selection_inputs
        {
            return last.listing.clone();
        }
        let next = listbox.listing(this, entry);
        let mut last = self.last.borrow_mut();
        let listing = match last.as_ref() {
            Some(last) if same_entries(&last.listing, &next) => last.listing.clone(),
            _ => Rc::new(next),
        };
        *last = Some(Last {
            inputs,
            selection: selection_inputs,
            listing: listing.clone(),
        });
        listing
    }
}

// Whether two listings hold the same entries.
fn same_entries(a: &HashMap<SharedString, SharedString>, b: &HashMap<SharedString, SharedString>) -> bool {
    a.len() == b.len() && a.iter().all(|(key, entry)| b.get(key) == Some(entry))
}
