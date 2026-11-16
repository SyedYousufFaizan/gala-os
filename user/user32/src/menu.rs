//! Menus as data: HMENU objects holding items (strings, separators,
//! popups with their submenus, ids, states, item data), with the
//! documented results of the menu functions. MF_BYCOMMAND reaches into
//! submenus. A window's menu (SetMenu/GetMenu) and its system menu
//! (GetSystemMenu: Restore, Move, Size, Minimize, Maximize, a separator,
//! Close) go with it. The menu bar takes the top of the window's client
//! area (`tree::set_menu_bar`); `track` draws it and the popups.

use crate::window::Hwnd;
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn VirtualAlloc(
        at: *mut core::ffi::c_void,
        size: usize,
        kind: u32,
        protect: u32,
    ) -> *mut core::ffi::c_void;
    fn VirtualFree(at: *mut core::ffi::c_void, size: usize, kind: u32) -> i32;
}

const MAX_MENUS: usize = 512;
const MAX_ITEMS: usize = 64;
const TEXT: usize = 64;
const HMENU_BASE: usize = 0x0003_0000;

const MF_GRAYED: u32 = 0x1;
const MF_DISABLED: u32 = 0x2;
const MF_BITMAP: u32 = 0x4;
const MF_CHECKED: u32 = 0x8;
const MF_POPUP: u32 = 0x10;
const MF_OWNERDRAW: u32 = 0x100;
const MF_BYPOSITION: u32 = 0x400;
const MF_SEPARATOR: u32 = 0x800;
const MF_HILITE: u32 = 0x80;
const MF_DEFAULT: u32 = 0x1000;
/// What an item's type keeps of the MF_ flags (the rest is state).
const TYPE_FLAGS: u32 = MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR | 0x20 | 0x40 | 0x200 | 0x4000;
const STATE_FLAGS: u32 = MF_GRAYED | MF_DISABLED | MF_CHECKED | MF_HILITE | MF_DEFAULT;

#[derive(Clone, Copy)]
struct Item {
    id: u32,
    /// MFT_ bits (MF_ type flags).
    kind: u32,
    /// MFS_ bits (MF_ state flags).
    state: u32,
    submenu: usize,
    data: usize,
    bitmap: usize,
    /// SetMenuItemBitmaps' bitmaps, drawn where the check mark goes
    /// (0: the check mark itself).
    checked: usize,
    unchecked: usize,
    text: [u16; TEXT],
}

const NO_ITEM: Item = Item {
    id: 0,
    kind: 0,
    state: 0,
    submenu: 0,
    data: 0,
    bitmap: 0,
    checked: 0,
    unchecked: 0,
    text: [0; TEXT],
};

struct Menu {
    popup: bool,
    count: usize,
    items: [Item; MAX_ITEMS],
    /// MENUINFO: style, maximum height, background brush, help id, data.
    style: u32,
    max_height: u32,
    background: usize,
    help_id: u32,
    data: usize,
}

struct Table {
    lock: AtomicBool,
    menus: core::cell::UnsafeCell<[*mut Menu; MAX_MENUS]>,
    /// (window, menu, system menu).
    windows: core::cell::UnsafeCell<[(Hwnd, usize, usize); 128]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    menus: core::cell::UnsafeCell::new([core::ptr::null_mut(); MAX_MENUS]),
    windows: core::cell::UnsafeCell::new([(0, 0, 0); 128]),
};

struct State<'a> {
    menus: &'a mut [*mut Menu; MAX_MENUS],
    windows: &'a mut [(Hwnd, usize, usize); 128],
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = unsafe {
        f(&mut State { menus: &mut *TABLE.menus.get(), windows: &mut *TABLE.windows.get() })
    };
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn slot(h: usize) -> Option<usize> {
    let d = h.checked_sub(HMENU_BASE)?;
    (d % 0x10 == 0 && d / 0x10 < MAX_MENUS).then_some(d / 0x10)
}

impl State<'_> {
    fn menu(&mut self, h: usize) -> Option<&mut Menu> {
        let p = *self.menus.get(slot(h)?)?;
        // SAFETY: a live menu's memory (freed only when its slot clears).
        (!p.is_null()).then(|| unsafe { &mut *p })
    }

    fn create(&mut self, popup: bool) -> usize {
        let Some(i) = self.menus.iter().position(|p| p.is_null()) else { return 0 };
        // SAFETY: a plain call; zeroed memory is an empty menu.
        let p = unsafe { VirtualAlloc(core::ptr::null_mut(), size_of::<Menu>(), 0x3000, 4) }
            .cast::<Menu>();
        if p.is_null() {
            return 0;
        }
        // SAFETY: fresh memory of the size of a Menu.
        unsafe { (*p).popup = popup };
        self.menus[i] = p;
        HMENU_BASE + i * 0x10
    }

    /// Destroys `h` and its submenus.
    fn destroy(&mut self, h: usize) -> bool {
        let Some(i) = slot(h) else { return false };
        let p = self.menus[i];
        if p.is_null() {
            return false;
        }
        self.menus[i] = core::ptr::null_mut();
        // SAFETY: the menu's memory, no longer reachable from the table.
        let subs: [usize; MAX_ITEMS] = unsafe { core::array::from_fn(|k| (*p).items[k].submenu) };
        // SAFETY: as above.
        let count = unsafe { (*p).count };
        // SAFETY: allocated by `create`.
        unsafe { VirtualFree(p.cast(), 0, 0x8000) };
        for &s in subs.iter().take(count) {
            if s != 0 {
                self.destroy(s);
            }
        }
        true
    }

    /// The item `which` of `h` (by position, or by command searching
    /// submenus): the menu that holds it and its index.
    fn find(&mut self, h: usize, which: u32, by_position: bool) -> Option<(usize, usize)> {
        let m = self.menu(h)?;
        if by_position {
            return ((which as usize) < m.count).then_some((h, which as usize));
        }
        let count = m.count;
        if let Some(k) = m.items.iter().take(count).position(|i| i.submenu == 0 && i.id == which) {
            return Some((h, k));
        }
        for k in 0..count {
            let sub = self.menu(h)?.items[k].submenu;
            if sub != 0
                && let Some(found) = self.find(sub, which, false)
            {
                return Some(found);
            }
        }
        None
    }

    fn item(&mut self, at: (usize, usize)) -> Option<&mut Item> {
        let m = self.menu(at.0)?;
        m.items.get_mut(at.1)
    }

    fn insert(&mut self, h: usize, at: usize, item: Item) -> bool {
        let Some(m) = self.menu(h) else { return false };
        if m.count == MAX_ITEMS {
            return false;
        }
        let at = at.min(m.count);
        m.items.copy_within(at..m.count, at + 1);
        m.items[at] = item;
        m.count += 1;
        true
    }

    fn remove(&mut self, at: (usize, usize)) -> Option<Item> {
        let m = self.menu(at.0)?;
        let item = m.items[at.1];
        m.items.copy_within(at.1 + 1..m.count, at.1);
        m.count -= 1;
        m.items[m.count] = NO_ITEM;
        Some(item)
    }
}

/// A NUL-terminated string (or NULL) as an item's text.
fn text_of(s: *const u16) -> [u16; TEXT] {
    let mut t = [0u16; TEXT];
    if s.is_null() {
        return t;
    }
    for (i, o) in t.iter_mut().take(TEXT - 1).enumerate() {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *s.add(i) };
        if c == 0 {
            break;
        }
        *o = c;
    }
    t
}

fn text_len(t: &[u16; TEXT]) -> usize {
    t.iter().position(|&c| c == 0).unwrap_or(TEXT)
}

/// An item from MF_ flags, an id (or submenu) and a string.
fn item_of(flags: u32, id: usize, text: *const u16) -> Item {
    let popup = flags & MF_POPUP != 0;
    let kind = flags & TYPE_FLAGS;
    Item {
        id: if popup { 0 } else { id as u32 },
        kind,
        state: flags & STATE_FLAGS,
        submenu: if popup { id } else { 0 },
        data: 0,
        bitmap: if flags & MF_BITMAP != 0 { text as usize } else { 0 },
        checked: 0,
        unchecked: 0,
        text: if kind & (MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR) == 0 {
            text_of(text)
        } else {
            [0; TEXT]
        },
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateMenu() -> usize {
    with(|s| s.create(false))
}

#[unsafe(no_mangle)]
pub extern "system" fn CreatePopupMenu() -> usize {
    with(|s| s.create(true))
}

#[unsafe(no_mangle)]
pub extern "system" fn DestroyMenu(h: usize) -> i32 {
    with(|s| i32::from(s.destroy(h)))
}

#[unsafe(no_mangle)]
pub extern "system" fn IsMenu(h: usize) -> i32 {
    with(|s| i32::from(s.menu(h).is_some()))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMenuItemCount(h: usize) -> i32 {
    with(|s| s.menu(h).map_or(-1, |m| m.count as i32))
}

#[unsafe(no_mangle)]
pub extern "system" fn AppendMenuW(h: usize, flags: u32, id: usize, text: *const u16) -> i32 {
    with(|s| i32::from(s.insert(h, usize::MAX, item_of(flags, id, text))))
}

#[unsafe(no_mangle)]
pub extern "system" fn AppendMenuA(h: usize, flags: u32, id: usize, text: *const u8) -> i32 {
    let mut w = [0u16; TEXT];
    let t = crate::window::ansi_into_ptr(text, &mut w);
    AppendMenuW(h, flags, id, t)
}

/// InsertMenuW(h, position, flags, id, text): before the item named by
/// position or command (-1 by position: at the end).
#[unsafe(no_mangle)]
pub extern "system" fn InsertMenuW(
    h: usize,
    position: u32,
    flags: u32,
    id: usize,
    text: *const u16,
) -> i32 {
    let item = item_of(flags, id, text);
    with(|s| {
        if flags & MF_BYPOSITION != 0 {
            let at = if position == u32::MAX { usize::MAX } else { position as usize };
            return i32::from(s.insert(h, at, item));
        }
        match s.find(h, position, false) {
            Some((menu, at)) => i32::from(s.insert(menu, at, item)),
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn ModifyMenuW(
    h: usize,
    position: u32,
    flags: u32,
    id: usize,
    text: *const u16,
) -> i32 {
    let new = item_of(flags, id, text);
    with(|s| {
        let Some(at) = s.find(h, position, flags & MF_BYPOSITION != 0) else { return 0 };
        let Some(item) = s.item(at) else { return 0 };
        let data = item.data;
        *item = Item { data, ..new };
        1
    })
}

/// RemoveMenu: the item goes, its submenu stays.
#[unsafe(no_mangle)]
pub extern "system" fn RemoveMenu(h: usize, position: u32, flags: u32) -> i32 {
    with(|s| match s.find(h, position, flags & MF_BYPOSITION != 0) {
        Some(at) => i32::from(s.remove(at).is_some()),
        None => 0,
    })
}

/// DeleteMenu: the item goes and its submenu is destroyed.
#[unsafe(no_mangle)]
pub extern "system" fn DeleteMenu(h: usize, position: u32, flags: u32) -> i32 {
    with(|s| match s.find(h, position, flags & MF_BYPOSITION != 0).and_then(|at| s.remove(at)) {
        Some(item) => {
            if item.submenu != 0 {
                s.destroy(item.submenu);
            }
            1
        }
        None => 0,
    })
}

/// GetMenuItemID: the id; -1 for a popup or no such item.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuItemID(h: usize, position: i32) -> u32 {
    with(|s| match s.find(h, position as u32, true).and_then(|at| s.item(at).copied()) {
        Some(item) if item.submenu == 0 => item.id,
        _ => u32::MAX,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSubMenu(h: usize, position: i32) -> usize {
    with(|s| {
        s.find(h, position as u32, true).and_then(|at| s.item(at).copied()).map_or(0, |i| i.submenu)
    })
}

/// GetMenuState: the item's state and type flags (a popup: its item count
/// in the high byte and MF_POPUP); -1 for no such item.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuState(h: usize, id: u32, flags: u32) -> u32 {
    with(|s| {
        let Some(item) =
            s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at).copied())
        else {
            return u32::MAX;
        };
        let bits = item.state | item.kind;
        if item.submenu != 0 {
            let count = s.menu(item.submenu).map_or(0, |m| m.count as u32);
            return (count << 8) | MF_POPUP | (bits & 0xff);
        }
        bits
    })
}

/// EnableMenuItem: MF_ENABLED/MF_GRAYED/MF_DISABLED; the previous ones
/// (-1 for no such item).
#[unsafe(no_mangle)]
pub extern "system" fn EnableMenuItem(h: usize, id: u32, flags: u32) -> u32 {
    with(|s| {
        let Some(item) = s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at)) else {
            return u32::MAX;
        };
        let previous = item.state & (MF_GRAYED | MF_DISABLED);
        item.state =
            (item.state & !(MF_GRAYED | MF_DISABLED)) | (flags & (MF_GRAYED | MF_DISABLED));
        previous
    })
}

/// CheckMenuItem: MF_CHECKED/MF_UNCHECKED; the previous (-1 for no such
/// item).
#[unsafe(no_mangle)]
pub extern "system" fn CheckMenuItem(h: usize, id: u32, flags: u32) -> u32 {
    with(|s| {
        let Some(item) = s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at)) else {
            return u32::MAX;
        };
        let previous = item.state & MF_CHECKED;
        item.state = (item.state & !MF_CHECKED) | (flags & MF_CHECKED);
        previous
    })
}

/// CheckMenuRadioItem(h, first, last, check, flags): checks one item of
/// the range and unchecks the others (by position or command).
#[unsafe(no_mangle)]
pub extern "system" fn CheckMenuRadioItem(
    h: usize,
    first: u32,
    last: u32,
    check: u32,
    flags: u32,
) -> i32 {
    let by_position = flags & MF_BYPOSITION != 0;
    with(|s| {
        let Some(m) = s.menu(h) else { return 0 };
        let count = m.count;
        let mut found = false;
        for (k, item) in m.items.iter_mut().take(count).enumerate() {
            let key = if by_position { k as u32 } else { item.id };
            if (first..=last).contains(&key) {
                if key == check {
                    item.state |= MF_CHECKED;
                    item.kind |= 0x200; // MFT_RADIOCHECK
                    found = true;
                } else {
                    item.state &= !MF_CHECKED;
                }
            }
        }
        i32::from(found)
    })
}

/// GetMenuStringW: the item's text (n - 1 characters at most); its length
/// without the NUL (the whole length when `buf` is NULL).
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuStringW(h: usize, id: u32, buf: *mut u16, n: i32, flags: u32) -> i32 {
    with(|s| {
        let Some(item) =
            s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at).copied())
        else {
            return 0;
        };
        let len = text_len(&item.text);
        if buf.is_null() || n <= 0 {
            return len as i32;
        }
        let k = len.min(n as usize - 1);
        // SAFETY: the caller's buffer of `n` characters.
        unsafe {
            core::ptr::copy_nonoverlapping(item.text.as_ptr(), buf, k);
            *buf.add(k) = 0;
        }
        k as i32
    })
}

/// MENUITEMINFOW (x64 layout).
#[repr(C)]
pub struct MenuItemInfoW {
    size: u32,
    mask: u32,
    kind: u32,
    state: u32,
    id: u32,
    submenu: usize,
    checked: usize,
    unchecked: usize,
    data: usize,
    type_data: *mut u16,
    cch: u32,
    bitmap: usize,
}

const MIIM_STATE: u32 = 0x1;
const MIIM_ID: u32 = 0x2;
const MIIM_SUBMENU: u32 = 0x4;
const MIIM_TYPE: u32 = 0x10;
const MIIM_DATA: u32 = 0x20;
const MIIM_STRING: u32 = 0x40;
const MIIM_CHECKMARKS: u32 = 0x8;
const MIIM_BITMAP: u32 = 0x80;
const MIIM_FTYPE: u32 = 0x100;

/// Applies a MENUITEMINFOW to an item.
fn apply(item: &mut Item, m: &MenuItemInfoW) {
    if m.mask & MIIM_STATE != 0 {
        item.state = m.state & STATE_FLAGS;
    }
    if m.mask & MIIM_ID != 0 {
        item.id = m.id;
    }
    if m.mask & MIIM_SUBMENU != 0 {
        item.submenu = m.submenu;
    }
    if m.mask & MIIM_DATA != 0 {
        item.data = m.data;
    }
    if m.mask & (MIIM_FTYPE | MIIM_TYPE) != 0 {
        item.kind = m.kind & TYPE_FLAGS;
    }
    if m.mask & MIIM_BITMAP != 0 {
        item.bitmap = m.bitmap;
    }
    if m.mask & MIIM_CHECKMARKS != 0 {
        item.checked = m.checked;
        item.unchecked = m.unchecked;
    }
    let string = m.mask & MIIM_STRING != 0
        || (m.mask & MIIM_TYPE != 0 && m.kind & (MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR) == 0);
    if string {
        item.text = text_of(m.type_data);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn InsertMenuItemW(
    h: usize,
    which: u32,
    by_position: i32,
    info: *const MenuItemInfoW,
) -> i32 {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's MENUITEMINFOW.
    let m = unsafe { &*info };
    let mut item = NO_ITEM;
    apply(&mut item, m);
    with(|s| {
        if by_position != 0 {
            let at = if which == u32::MAX { usize::MAX } else { which as usize };
            return i32::from(s.insert(h, at, item));
        }
        match s.find(h, which, false) {
            Some((menu, at)) => i32::from(s.insert(menu, at, item)),
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn SetMenuItemInfoW(
    h: usize,
    which: u32,
    by_position: i32,
    info: *const MenuItemInfoW,
) -> i32 {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's MENUITEMINFOW.
    let m = unsafe { &*info };
    with(|s| match s.find(h, which, by_position != 0).and_then(|at| s.item(at)) {
        Some(item) => {
            apply(item, m);
            1
        }
        None => 0,
    })
}

/// GetMenuItemInfoW: what the mask asks for; strings into dwTypeData (cch
/// characters at most), cch becoming the length without the NUL.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuItemInfoW(
    h: usize,
    which: u32,
    by_position: i32,
    info: *mut MenuItemInfoW,
) -> i32 {
    if info.is_null() {
        return 0;
    }
    let Some(item) =
        with(|s| s.find(h, which, by_position != 0).and_then(|at| s.item(at).copied()))
    else {
        return 0;
    };
    // SAFETY: the caller's MENUITEMINFOW.
    let m = unsafe { &mut *info };
    if m.mask & MIIM_STATE != 0 {
        m.state = item.state;
    }
    if m.mask & MIIM_ID != 0 {
        m.id = item.id;
    }
    if m.mask & MIIM_SUBMENU != 0 {
        m.submenu = item.submenu;
    }
    if m.mask & MIIM_DATA != 0 {
        m.data = item.data;
    }
    if m.mask & (MIIM_FTYPE | MIIM_TYPE) != 0 {
        m.kind = item.kind;
    }
    if m.mask & MIIM_BITMAP != 0 {
        m.bitmap = item.bitmap;
    }
    if m.mask & MIIM_CHECKMARKS != 0 {
        m.checked = item.checked;
        m.unchecked = item.unchecked;
    }
    if m.mask & (MIIM_STRING | MIIM_TYPE) != 0 {
        let len = text_len(&item.text);
        if !m.type_data.is_null() && m.cch > 0 {
            let k = len.min(m.cch as usize - 1);
            // SAFETY: the caller's buffer of cch characters.
            unsafe {
                core::ptr::copy_nonoverlapping(item.text.as_ptr(), m.type_data, k);
                *m.type_data.add(k) = 0;
            }
        }
        m.cch = len as u32;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn SetMenuDefaultItem(h: usize, which: u32, by_position: u32) -> i32 {
    with(|s| {
        let Some(m) = s.menu(h) else { return 0 };
        let count = m.count;
        for (k, item) in m.items.iter_mut().take(count).enumerate() {
            let key = if by_position != 0 { k as u32 } else { item.id };
            if key == which {
                item.state |= MF_DEFAULT;
            } else {
                item.state &= !MF_DEFAULT;
            }
        }
        1
    })
}

/// GetMenuCheckMarkDimensions: the size of a menu item's check mark, the
/// width in the low word and the height in the high one (SM_CXMENUCHECK
/// and SM_CYMENUCHECK).
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuCheckMarkDimensions() -> i32 {
    let (cx, cy) = (crate::window::GetSystemMetrics(71), crate::window::GetSystemMetrics(72));
    (((cy as u32) << 16) | (cx as u32 & 0xffff)) as i32
}

const GMDI_USEDISABLED: u32 = 0x0001;
const GMDI_GOINTOPOPUPS: u32 = 0x0002;
/// What the menu functions answer for "no item".
const NONE: u32 = u32::MAX;

/// The item SetMenuDefaultItem marked, by position or by command.
fn default_item(s: &mut State, h: usize, by_position: u32, flags: u32) -> u32 {
    let Some(menu) = s.menu(h) else { return NONE };
    let count = menu.count;
    let Some(k) = menu.items.iter().take(count).position(|i| i.state & MF_DEFAULT != 0) else {
        return NONE;
    };
    let item = menu.items[k];
    if item.state & (MF_GRAYED | MF_DISABLED) != 0 && flags & GMDI_USEDISABLED == 0 {
        return NONE;
    }
    if item.submenu != 0 {
        // GMDI_GOINTOPOPUPS asks for the default of the popup's submenu
        // instead; without it, a popup has no command of its own and
        // Windows answers with the submenu's handle.
        if flags & GMDI_GOINTOPOPUPS != 0 {
            let inner = default_item(s, item.submenu, by_position, flags);
            if inner != NONE {
                return inner;
            }
        }
        return if by_position != 0 { k as u32 } else { item.submenu as u32 };
    }
    if by_position != 0 { k as u32 } else { item.id }
}

/// GetMenuDefaultItem: the default item's position (`by_position`) or
/// command; -1 when the menu has none, or when the default is disabled
/// and GMDI_USEDISABLED was not given.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuDefaultItem(h: usize, by_position: u32, flags: u32) -> u32 {
    with(|s| default_item(s, h, by_position, flags))
}

/// SetMenuItemBitmaps: the bitmaps drawn where the item's check mark
/// goes, unchecked and checked (NULL for either puts the check mark
/// back). The item keeps them (MIIM_CHECKMARKS); `track` draws the check
/// mark, not the bitmaps, which is what the ledger records here.
#[unsafe(no_mangle)]
pub extern "system" fn SetMenuItemBitmaps(
    h: usize,
    which: u32,
    flags: u32,
    unchecked: usize,
    checked: usize,
) -> i32 {
    if unchecked != 0 || checked != 0 {
        crate::record(b"user32.dll!SetMenuItemBitmaps(not drawn)", which.into());
    }
    with(|s| {
        let Some(at) = s.find(h, which, flags & MF_BYPOSITION != 0) else { return 0 };
        let Some(item) = s.item(at) else { return 0 };
        item.unchecked = unchecked;
        item.checked = checked;
        1
    })
}

// ---- Menus from RT_MENU resources ----

/// Reads a menu template.
struct Reader {
    at: *const u8,
    /// The resource's first byte: MENUEX items start on a DWORD boundary
    /// from it.
    base: *const u8,
}

impl Reader {
    /// # Safety
    /// The template holds the bytes read, which a well-formed one does
    /// up to the item that ends the outermost list.
    unsafe fn u16(&mut self) -> u16 {
        // SAFETY: the caller's promise.
        let v = unsafe { self.at.cast::<u16>().read_unaligned() };
        self.at = self.at.wrapping_add(2);
        v
    }

    /// # Safety
    /// As `u16`.
    unsafe fn u32(&mut self) -> u32 {
        // SAFETY: the caller's promise.
        let v = unsafe { self.at.cast::<u32>().read_unaligned() };
        self.at = self.at.wrapping_add(4);
        v
    }

    /// The NUL-terminated string at the cursor (truncated to what an
    /// item holds), the cursor left after its NUL.
    ///
    /// # Safety
    /// As `u16`.
    unsafe fn text(&mut self) -> [u16; TEXT] {
        let mut out = [0u16; TEXT];
        let mut k = 0;
        loop {
            // SAFETY: the caller's promise.
            let c = unsafe { self.u16() };
            if c == 0 {
                return out;
            }
            if k < TEXT - 1 {
                out[k] = c;
                k += 1;
            }
        }
    }

    /// To the next DWORD boundary from the resource's start.
    fn align(&mut self) {
        let off = self.at as usize - self.base as usize;
        self.at = self.at.wrapping_add((4 - off % 4) % 4);
    }
}

/// MF_END in a template (MF_HILITE's bit, which a template does not
/// carry).
const MF_END: u32 = 0x80;
/// MFR_POPUP and MFR_END in a MENUEX item.
const MFR_POPUP: u16 = 0x01;
const MFR_END: u16 = 0x80;
/// How deep submenus are read.
const MAX_DEPTH: u32 = 8;

/// Reads the items of one menu (and its submenus) into `h`, version 0
/// items when `ex` is false and MENUEX items when it is. Answers whether
/// the template was well-formed as far as it was read.
///
/// # Safety
/// `r` is on the first item of a template of that version.
unsafe fn read_items(s: &mut State, r: &mut Reader, h: usize, ex: bool, depth: u32) -> bool {
    if depth > MAX_DEPTH {
        crate::record(b"user32.dll!LoadMenuW(nesting)", depth.into());
        return false;
    }
    for _ in 0..MAX_ITEMS {
        // SAFETY: the caller's promise.
        let (flags, id, last) = unsafe {
            if ex {
                r.align();
                let kind = r.u32();
                let state = r.u32();
                let id = r.u32() as usize;
                let info = r.u16();
                let mut flags = (kind & TYPE_FLAGS) | (state & STATE_FLAGS);
                if info & MFR_POPUP != 0 {
                    flags |= MF_POPUP;
                }
                (flags, id, info & MFR_END != 0)
            } else {
                let option = u32::from(r.u16());
                let id = if option & MF_POPUP != 0 { 0 } else { usize::from(r.u16()) };
                (option & !MF_END, id, option & MF_END != 0)
            }
        };
        // SAFETY: the caller's promise.
        let text = unsafe { r.text() };
        let mut item = item_of(flags & !MF_POPUP, id, text.as_ptr());
        if flags & MF_POPUP != 0 {
            if ex {
                r.align();
                // SAFETY: the caller's promise: a popup's help id.
                unsafe { r.u32() };
            }
            let sub = s.create(true);
            // SAFETY: the caller's promise; the submenu's items follow.
            if sub == 0 || !unsafe { read_items(s, r, sub, ex, depth + 1) } {
                return false;
            }
            item.submenu = sub;
            item.id = 0;
        }
        if !s.insert(h, MAX_ITEMS, item) {
            return false;
        }
        if last {
            return true;
        }
    }
    crate::record(b"user32.dll!LoadMenuW(items)", MAX_ITEMS as u64);
    false
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn FindResourceW(module: usize, name: usize, kind: usize) -> usize;
    fn LoadResource(module: usize, res: usize) -> *const u16;
}

/// LoadMenuW: the menu of the module's RT_MENU resource `name` (an
/// integer id in the low word, or a string); NULL when the module has no
/// such resource, or when its template is not one gala can read (the
/// ledger says which). Both template versions are read: version 0
/// (flags, id and text per item) and version 1 (MENUEX: type, state, id
/// and flags, each item on a DWORD boundary and a popup's help id after
/// its text). What MENUEX adds beyond the MF_ flags (an item's own help
/// id) is dropped.
#[unsafe(no_mangle)]
pub extern "system" fn LoadMenuW(instance: usize, name: *const u16) -> usize {
    const RT_MENU: usize = 4;
    // SAFETY: plain calls on the caller's module and resource name.
    let data = unsafe {
        let res = FindResourceW(instance, name as usize, RT_MENU);
        if res == 0 {
            return 0;
        }
        LoadResource(instance, res).cast::<u8>()
    };
    if data.is_null() {
        return 0;
    }
    with(|s| {
        let h = s.create(false);
        if h == 0 {
            return 0;
        }
        let mut r = Reader { at: data, base: data };
        // SAFETY: the module's own resource, read as its version says.
        let ok = unsafe {
            let version = r.u16();
            let offset = r.u16();
            match version {
                0 => {
                    r.at = data.wrapping_add(4 + usize::from(offset));
                    read_items(s, &mut r, h, false, 0)
                }
                1 => {
                    r.at = data.wrapping_add(4 + usize::from(offset));
                    read_items(s, &mut r, h, true, 0)
                }
                _ => {
                    crate::record(b"user32.dll!LoadMenuW(version)", version.into());
                    false
                }
            }
        };
        if !ok {
            s.destroy(h);
            return 0;
        }
        h
    })
}

// ---- Windows' menus ----

fn entry<'a>(s: &'a mut State, hwnd: Hwnd) -> Option<&'a mut (Hwnd, usize, usize)> {
    if let Some(i) = s.windows.iter().position(|e| e.0 == hwnd) {
        return Some(&mut s.windows[i]);
    }
    let i = s.windows.iter().position(|e| e.0 == 0)?;
    s.windows[i] = (hwnd, 0, 0);
    Some(&mut s.windows[i])
}

/// SetMenu: the window's menu bar (NULL: none); the client area gives
/// the bar its room (or takes it back).
#[unsafe(no_mangle)]
pub extern "system" fn SetMenu(hwnd: Hwnd, menu: usize) -> i32 {
    if crate::window::IsWindow(hwnd) == 0 || attach(hwnd, menu) == 0 {
        return 0;
    }
    crate::tree::set_menu_bar(hwnd, menu != 0);
    1
}

/// The menu goes with the window (no layout).
pub(crate) fn attach(hwnd: Hwnd, menu: usize) -> i32 {
    with(|s| {
        if menu != 0 && s.menu(menu).is_none() {
            return 0;
        }
        match entry(s, hwnd) {
            Some(e) => {
                e.1 = menu;
                1
            }
            None => 0,
        }
    })
}

/// An item as the menu tracker draws it.
#[derive(Clone, Copy)]
pub(crate) struct View {
    pub id: u32,
    pub separator: bool,
    pub disabled: bool,
    pub checked: bool,
    pub radio: bool,
    pub owner_draw: bool,
    pub submenu: usize,
    pub data: usize,
    pub text: [u16; TEXT],
    pub len: usize,
}

/// How many items `h` has (0 for no menu).
pub(crate) fn count(h: usize) -> usize {
    with(|s| s.menu(h).map_or(0, |m| m.count))
}

/// Item `k` of `h`.
pub(crate) fn view(h: usize, k: usize) -> Option<View> {
    const MFT_RADIOCHECK: u32 = 0x200;
    with(|s| {
        let m = s.menu(h)?;
        let it = *m.items.get(k).filter(|_| k < m.count)?;
        Some(View {
            id: it.id,
            separator: it.kind & MF_SEPARATOR != 0,
            disabled: it.state & (MF_GRAYED | MF_DISABLED) != 0,
            checked: it.state & MF_CHECKED != 0,
            radio: it.kind & MFT_RADIOCHECK != 0,
            owner_draw: it.kind & MF_OWNERDRAW != 0,
            submenu: it.submenu,
            data: it.data,
            text: it.text,
            len: text_len(&it.text),
        })
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMenu(hwnd: Hwnd) -> usize {
    with(|s| s.windows.iter().find(|e| e.0 == hwnd && hwnd != 0).map_or(0, |e| e.1))
}

/// DrawMenuBar: the window's menu bar again (after its items changed).
#[unsafe(no_mangle)]
pub extern "system" fn DrawMenuBar(hwnd: Hwnd) -> i32 {
    if crate::window::IsWindow(hwnd) == 0 {
        return 0;
    }
    crate::track::draw_bar(hwnd);
    1
}

/// GetSystemMenu(hwnd, revert): the window's own copy of the window menu
/// (made on first use); revert: back to the default, NULL.
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemMenu(hwnd: Hwnd, revert: i32) -> usize {
    if crate::window::IsWindow(hwnd) == 0 {
        return 0;
    }
    let existing = with(|s| entry(s, hwnd).map_or(0, |e| e.2));
    if revert != 0 {
        if existing != 0 {
            with(|s| {
                s.destroy(existing);
                if let Some(e) = entry(s, hwnd) {
                    e.2 = 0;
                }
            });
        }
        return 0;
    }
    if existing != 0 {
        return existing;
    }
    let m = CreatePopupMenu();
    let items: [(u32, &[u8]); 7] = [
        (0xf120, b"&Restore"),
        (0xf010, b"&Move"),
        (0xf000, b"&Size"),
        (0xf020, b"Mi&nimize"),
        (0xf030, b"Ma&ximize"),
        (0, b""),
        (0xf060, b"&Close\tAlt+F4"),
    ];
    for (id, text) in items {
        if id == 0 {
            AppendMenuW(m, MF_SEPARATOR, 0, core::ptr::null());
            continue;
        }
        let mut w = [0u16; 32];
        for (o, &c) in w.iter_mut().zip(text) {
            *o = u16::from(c);
        }
        AppendMenuW(m, 0, id as usize, w.as_ptr());
    }
    with(|s| {
        if let Some(e) = entry(s, hwnd) {
            e.2 = m;
        }
    });
    m
}

/// A window went: its menu and system menu are destroyed with it.
pub fn window_destroyed(hwnd: Hwnd) {
    with(|s| {
        if let Some(i) = s.windows.iter().position(|e| e.0 == hwnd) {
            let (_, menu, system) = s.windows[i];
            s.windows[i] = (0, 0, 0);
            for m in [menu, system] {
                if m != 0 {
                    s.destroy(m);
                }
            }
        }
    });
}

/// MENUINFO (x64 layout).
#[repr(C)]
pub struct MenuInfo {
    size: u32,
    mask: u32,
    style: u32,
    max_height: u32,
    background: usize,
    help_id: u32,
    data: usize,
}

const MIM_MAXHEIGHT: u32 = 0x1;
const MIM_BACKGROUND: u32 = 0x2;
const MIM_HELPID: u32 = 0x4;
const MIM_MENUDATA: u32 = 0x8;
const MIM_STYLE: u32 = 0x10;
const MIM_APPLYTOSUBMENUS: u32 = 0x8000_0000;

/// SetMenuInfo: the fields the mask names (MIM_APPLYTOSUBMENUS: to the
/// submenus too).
#[unsafe(no_mangle)]
pub extern "system" fn SetMenuInfo(h: usize, info: *const MenuInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's MENUINFO.
    let mi = unsafe { &*info };
    if mi.size as usize != size_of::<MenuInfo>() {
        crate::window::set_last_error(87);
        return 0;
    }
    fn apply(s: &mut State, h: usize, mi: &MenuInfo, depth: u32) -> bool {
        let Some(m) = s.menu(h) else { return false };
        if mi.mask & MIM_STYLE != 0 {
            m.style = mi.style;
        }
        if mi.mask & MIM_MAXHEIGHT != 0 {
            m.max_height = mi.max_height;
        }
        if mi.mask & MIM_BACKGROUND != 0 {
            m.background = mi.background;
        }
        if mi.mask & MIM_HELPID != 0 {
            m.help_id = mi.help_id;
        }
        if mi.mask & MIM_MENUDATA != 0 {
            m.data = mi.data;
        }
        if mi.mask & MIM_APPLYTOSUBMENUS != 0 && depth < 16 {
            let count = m.count;
            for k in 0..count {
                let sub = s.menu(h).map_or(0, |m| m.items[k].submenu);
                if sub != 0 {
                    apply(s, sub, mi, depth + 1);
                }
            }
        }
        true
    }
    with(|s| i32::from(apply(s, h, mi, 0)))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMenuInfo(h: usize, info: *mut MenuInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's MENUINFO.
    let mi = unsafe { &mut *info };
    with(|s| {
        let Some(m) = s.menu(h) else { return 0 };
        if mi.mask & MIM_STYLE != 0 {
            mi.style = m.style;
        }
        if mi.mask & MIM_MAXHEIGHT != 0 {
            mi.max_height = m.max_height;
        }
        if mi.mask & MIM_BACKGROUND != 0 {
            mi.background = m.background;
        }
        if mi.mask & MIM_HELPID != 0 {
            mi.help_id = m.help_id;
        }
        if mi.mask & MIM_MENUDATA != 0 {
            mi.data = m.data;
        }
        1
    })
}

/// EndMenu: closes the active menu; gala does not track menus on the
/// screen yet, so there is none to close.
#[unsafe(no_mangle)]
pub extern "system" fn EndMenu() -> i32 {
    1
}
