use crate::*;

// ----- ext_data_control_manager_v1 (staging) --------------------------------
//
// Clipboard manager access: set and read the per-seat selection without a
// focused surface. This is the protocol wl-clipboard & co. prefer, precisely
// because wl_data_device.set_selection demands keyboard focus and forced them
// to create an invisible 1x1 helper toplevel to obtain it (ADR-0133). With
// this global advertised, those clients stop creating the helper window and
// the window switcher / FSP paths stop seeing phantom "wl-clipboard" windows.
//
// Semantics implemented here:
// - `create_data_source` / `offer`: a MIME-collecting source record, mirroring
//   `wl_data_source` minus the drag-and-drop surface.
// - `get_data_device(seat)`: a per-seat control device bound to the routed
//   seat, advertised immediately with the current selection.
// - `set_selection`: adopts the source into the same per-seat selection slot
//   `wl_data_device` uses, then notifies both protocol families. There is no
//   keyboard-focus precondition: possession of a bound data-control device for
//   the seat is the authority.
// - `receive` on an offer: forwards to the source's `send`, or serves
//   compositor-owned payloads through the bounded writer.
// - `finished`: posted when the owning seat's runtime is torn down; the client
//   must destroy the device then.
//
// Primary selection (device v1 events include `primary_selection`) is mapped
// onto the same clipboard: tessera models exactly one selection per seat. The
// dedicated primary-selection protocols remain unadvertised, so nothing can
// observe a divergence.
//
// No drag-and-drop: `start_drag` does not exist in this protocol. Offers
// created here are always selection offers.

static DATA_CONTROL_MANAGER_IMPL: ffi::ext_data_control_manager_v1_interface_impl =
    ffi::ext_data_control_manager_v1_interface_impl {
        create_data_source: dcm_create_data_source,
        get_data_device: dcm_get_data_device,
        destroy: res_destroy,
    };

static DATA_CONTROL_DEVICE_IMPL: ffi::ext_data_control_device_v1_interface_impl =
    ffi::ext_data_control_device_v1_interface_impl {
        set_selection: dcdev_set_selection,
        destroy: res_destroy,
        // tessera models exactly one selection per seat (no independent
        // primary clipboard), and the protocol says a compositor that does
        // not support primary selection simply never sends
        // `primary_selection`. Accepting `set_primary_selection` as an alias
        // would corrupt the regular clipboard: a manager clearing the
        // primary selection (e.g. `wl-copy -p --clear`) would clear the
        // user's Ctrl+C clipboard too. Refuse the request with the protocol
        // error reserved for it, so clients fall back to "unsupported".
        set_primary_selection: dcdev_set_primary_selection_unsupported,
    };
static DATA_CONTROL_SOURCE_IMPL: ffi::ext_data_control_source_v1_interface_impl =
    ffi::ext_data_control_source_v1_interface_impl {
        offer: dc_source_offer,
        destroy: res_destroy,
    };

static DATA_CONTROL_OFFER_IMPL: ffi::ext_data_control_offer_v1_interface_impl =
    ffi::ext_data_control_offer_v1_interface_impl {
        receive: dc_offer_receive,
        destroy: res_destroy,
    };

pub(crate) unsafe extern "C" fn data_control_manager_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let _ = client;
        let res = ffi::wl_resource_create(
            client,
            &ffi::ext_data_control_manager_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        // The manager itself owns no seat state; the data device resolves the
        // State from the wl_seat resource at get_data_device time.
        ffi::wl_resource_set_implementation(
            res,
            &DATA_CONTROL_MANAGER_IMPL as *const _ as *const c_void,
            _data,
            None,
        );
    }
}

/// State carried by one `ext_data_control_device_v1`.
struct DataControlDeviceRec {
    state: *mut State,
    /// Routed seat this device controls. `None` once the runtime is gone; the
    /// device is inert then and `finished` has been posted.
    seat: Option<SeatId>,
}

/// State carried by one `ext_data_control_source_v1`. Same shape as the
/// `wl_data_source` record minus DnD fields. No seat is recorded: a source
/// is client-owned state until a device adopts it, and the adopting device
/// (not the manager that created it) determines the seat. `seat_for_resource`
/// resolves the seat after adoption.
struct DataControlSourceRec {
    state: *mut State,
    mime_types: Vec<String>,
    /// The protocol forbids reusing a source in a second set_selection.
    used: bool,
}

/// State carried by one `ext_data_control_offer_v1`.
pub(crate) struct DataControlOfferRec {
    state: *mut State,
    /// Back-pointer to the source the selection was adopted from; null once
    /// the source is destroyed (a late `receive` then fails closed).
    pub(crate) source: *mut ffi::wl_resource,
    /// Interface family of `source` — see [`SelectionSourceKind`]. The
    /// selection a manager reads may have been set by either protocol
    /// family; `receive` must marshal `send` for the right one.
    pub(crate) source_kind: SelectionSourceKind,
    owned: Option<OwnedSelection>,
}

unsafe extern "C" fn dcm_create_data_source(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        if state.is_null() {
            return;
        }
        let src = ffi::wl_resource_create(
            client,
            &ffi::ext_data_control_source_v1_interface,
            ffi::wl_resource_get_version(mgr),
            id,
        );
        if src.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(DataControlSourceRec {
            state,
            mime_types: Vec::new(),
            used: false,
        }));
        ffi::wl_resource_set_implementation(
            src,
            &DATA_CONTROL_SOURCE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(dc_source_resource_destroy),
        );
    }
}

unsafe extern "C" fn dc_source_offer(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut DataControlSourceRec;
        if rec.is_null() || mime_type.is_null() {
            return;
        }
        if (*rec).used {
            // offer after set_selection is a protocol error per the XML.
            ffi::wl_resource_post_error(
                resource,
                ffi::EXT_DATA_CONTROL_SOURCE_V1_ERROR_INVALID_OFFER,
                c"offer sent after ext_data_control_device.set_selection".as_ptr(),
            );
            return;
        }
        if let Ok(s) = CStr::from_ptr(mime_type).to_str()
            && !(*rec).mime_types.iter().any(|m| m == s)
        {
            (*rec).mime_types.push(s.to_string());
        }
    }
}

unsafe extern "C" fn dc_source_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut DataControlSourceRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        if !state.is_null() {
            // The seat is only known after a device adopted this source
            // (`track_seat_resource` in set_selection); an unused source owns
            // no selection anywhere.
            if let Some(seat) = (*state).seat_for_resource(resource)
                && let Some(_guard) = ActiveSeatGuard::enter_existing(&mut *state, seat)
            {
                // A destroyed source relinquishes the selection it owns.
                if (*state)
                    .selection
                    .as_ref()
                    .is_some_and(|selection| selection.source == resource)
                {
                    replace_clipboard_selection(state, None);
                }
                // Offers are client-owned and can outlive their source. Both
                // families may have built offers from this source; null their
                // back-pointers so a late receive fails closed instead of
                // addressing freed memory.
                for offer in (*state)
                    .data_control_offers
                    .iter()
                    .copied()
                    .filter(|p| !p.is_null())
                {
                    let offer_rec =
                        ffi::wl_resource_get_user_data(offer) as *mut DataControlOfferRec;
                    if !offer_rec.is_null() && (*offer_rec).source == resource {
                        (*offer_rec).source = std::ptr::null_mut();
                    }
                }
                for offer in (*state)
                    .data_offers
                    .iter()
                    .copied()
                    .filter(|p| !p.is_null())
                {
                    let offer_rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
                    if !offer_rec.is_null() && (*offer_rec).source == resource {
                        (*offer_rec).source = std::ptr::null_mut();
                    }
                }
                (*state).untrack_seat_resource(resource);
            }
        }
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn dcm_get_data_device(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    seat: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(seat) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(advertised_seat) = (*state).seat_for_resource(seat) else {
            return;
        };
        let Some(_guard) = ActiveSeatGuard::for_client_seat_resource(state, client, seat, true)
        else {
            return;
        };
        let routed = (*state).active_seat;
        let dev = ffi::wl_resource_create(
            client,
            &ffi::ext_data_control_device_v1_interface,
            ffi::wl_resource_get_version(mgr),
            id,
        );
        if dev.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(DataControlDeviceRec {
            state,
            seat: Some(routed),
        }));
        ffi::wl_resource_set_implementation(
            dev,
            &DATA_CONTROL_DEVICE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(dc_device_resource_destroy),
        );
        (*state).track_routed_seat_resource(dev, advertised_seat, routed);
        if let Some(runtime) = (*state).seat_runtime_mut(routed) {
            runtime.data_control_devices.push(dev);
        }
        // Advertise the current selection immediately: a manager connects to
        // *read* the clipboard, focus or not.
        advertise_current_selection(state, dev, routed);
    }
}

/// Remove a data-control device resource from every seat runtime's device
/// list. The destroy path cannot assume the original seat still exists
/// (`finished` may have been posted and the rec's seat nulled), so it scrubs
/// everywhere rather than addressing one runtime.
unsafe fn scrub_data_control_device(state: &mut State, resource: *mut ffi::wl_resource) {
    for runtime in state.seats.values_mut() {
        runtime.data_control_devices.retain(|p| *p != resource);
    }
    state.untrack_seat_resource(resource);
}

/// Test-only re-export of the device scrub: the destroy handler resolves its
/// rec through libwayland user data, which unit tests cannot synthesise.
#[cfg(test)]
pub(crate) unsafe fn scrub_data_control_device_for_test(
    state: &mut State,
    resource: *mut ffi::wl_resource,
) {
    unsafe { scrub_data_control_device(state, resource) }
}

unsafe extern "C" fn dc_device_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut DataControlDeviceRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        // The seat may already be gone (`finished` was posted and the field
        // nulled): scrub the resource from *every* runtime that might still
        // list it. A stale entry here is a dangling pointer the next
        // selection change would post to.
        if !state.is_null() {
            scrub_data_control_device(&mut *state, resource);
        }
        drop(Box::from_raw(rec));
    }
}

/// `set_primary_selection`: tessera models exactly one selection per seat (no
/// independent primary clipboard), so per the protocol it never sends
/// `primary_selection`. The request is ignored rather than aliased onto the
/// regular selection: accepting it would corrupt the user's clipboard (a
/// manager clearing the primary selection, e.g. `wl-copy -p --clear`, would
/// clear the Ctrl+C clipboard too). Clients observe "primary unsupported"
/// from the absence of the initial `primary_selection` event.
unsafe extern "C" fn dcdev_set_primary_selection_unsupported(
    _client: *mut ffi::wl_client,
    device: *mut ffi::wl_resource,
    _source: *mut ffi::wl_resource,
) {
    let _ = device;
}

unsafe extern "C" fn dcdev_set_selection(
    client: *mut ffi::wl_client,
    device: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(device) as *mut DataControlDeviceRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        let Some(seat) = (*rec).seat else {
            return;
        };
        let Some(_guard) = ActiveSeatGuard::enter_existing(&mut *state, seat) else {
            return;
        };
        if !source.is_null() && ffi::wl_resource_get_client(source) != client {
            return;
        }
        if source.is_null() {
            replace_clipboard_selection(state, None);
            return;
        }
        let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataControlSourceRec;
        if source_rec.is_null() {
            return;
        }
        if (*source_rec).used {
            ffi::wl_resource_post_error(
                device,
                ffi::EXT_DATA_CONTROL_DEVICE_V1_ERROR_USED_SOURCE,
                c"source given to set_selection was already used before".as_ptr(),
            );
            return;
        }
        (*source_rec).used = true;
        let sel = Selection {
            source,
            source_kind: SelectionSourceKind::DataControl,
            mime_types: (*source_rec).mime_types.clone(),
            owned: None,
        };
        (*state).track_seat_resource(source, seat);
        replace_clipboard_selection(state, Some(sel));
    }
}

unsafe extern "C" fn dc_offer_receive(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
    fd: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(offer) as *mut DataControlOfferRec;
        if rec.is_null() {
            if fd >= 0 {
                libc_close(fd);
            }
            return;
        }
        if let Some(owned) = &(*rec).owned {
            let mime = if mime_type.is_null() {
                None
            } else {
                CStr::from_ptr(mime_type).to_str().ok()
            };
            if let Some(bytes) = mime.and_then(|mime| owned.payload(mime)) {
                crate::server::queue_owned_clipboard_write(fd, bytes);
            } else if fd >= 0 {
                libc_close(fd);
            }
            return;
        }
        let source = (*rec).source;
        if source.is_null() {
            if fd >= 0 {
                libc_close(fd);
            }
            return;
        }
        // Marshal `send` for the interface the source actually is: a GUI
        // app's wl_data_source (Ctrl+C) has different opcodes than a
        // data-control source, and posting the wrong one desynchronises the
        // client's protocol stream.
        let opcode = match (*rec).source_kind {
            SelectionSourceKind::WlDataSource => ffi::WL_DATA_SOURCE_SEND,
            SelectionSourceKind::DataControl => ffi::EXT_DATA_CONTROL_SOURCE_V1_SEND,
        };
        ffi::wl_resource_post_event(source, opcode, mime_type, fd);
        if fd >= 0 {
            libc_close(fd);
        }
    }
}

/// Build and send `data_offer` + `selection` for the current selection to one
/// data-control device. `null` selection is sent as `selection(null)`.
unsafe fn advertise_current_selection(state: *mut State, dev: *mut ffi::wl_resource, seat: SeatId) {
    unsafe {
        let client = ffi::wl_resource_get_client(dev);
        let version = ffi::wl_resource_get_version(dev);
        // `Selection` is not `Clone` (its raw source pointer must not be
        // duplicated casually); project the fields an offer needs.
        let selection = (*state)
            .seat_runtime(seat)
            .and_then(|r| r.selection.as_ref())
            .map(|sel| {
                (
                    sel.source,
                    sel.source_kind,
                    sel.mime_types.clone(),
                    sel.owned.clone(),
                )
            });
        let offer = match &selection {
            Some(sel) => {
                let offer = ffi::wl_resource_create(
                    client,
                    &ffi::ext_data_control_offer_v1_interface,
                    version,
                    0,
                );
                if offer.is_null() {
                    return;
                }
                let rec = Box::into_raw(Box::new(DataControlOfferRec {
                    state,
                    source: sel.0,
                    source_kind: sel.1,
                    owned: sel.3.clone(),
                }));
                ffi::wl_resource_set_implementation(
                    offer,
                    &DATA_CONTROL_OFFER_IMPL as *const _ as *const c_void,
                    rec as *mut c_void,
                    Some(dc_offer_resource_destroy),
                );
                if let Some(runtime) = (*state).seat_runtime_mut(seat) {
                    runtime.data_control_offers.push(offer);
                }
                // Track the offer like any seat-routed resource so its
                // destroy handler can find (and remove it from) the owning
                // runtime regardless of which dispatch batch unmaps it.
                (*state).track_routed_seat_resource(offer, seat, seat);
                ffi::wl_resource_post_event(dev, ffi::EXT_DATA_CONTROL_DEVICE_V1_DATA_OFFER, offer);
                for mime in &sel.2 {
                    if let Ok(c) = CString::new(mime.as_str()) {
                        ffi::wl_resource_post_event(
                            offer,
                            ffi::EXT_DATA_CONTROL_OFFER_V1_OFFER,
                            c.as_ptr(),
                        );
                    }
                }
                offer
            }
            None => std::ptr::null_mut(),
        };
        ffi::wl_resource_post_event(dev, ffi::EXT_DATA_CONTROL_DEVICE_V1_SELECTION, offer);
    }
}

unsafe extern "C" fn dc_offer_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut DataControlOfferRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        // Scrub from every runtime: an offer can outlive the seat runtime it
        // was registered on (seat quiesced/revoked), and a stale entry is a
        // dangling pointer on the next list walk.
        if !state.is_null() {
            for runtime in (*state).seats.values_mut() {
                runtime.data_control_offers.retain(|p| *p != resource);
            }
            (*state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}
/// Push the current selection to every live data-control device on `seat`.
/// Mirrors `notify_selection_changed` for the wl_data_device family.
pub(crate) unsafe fn notify_data_control_selection(state: *mut State, seat: SeatId) {
    unsafe {
        let devices: Vec<*mut ffi::wl_resource> = match (*state).seat_runtime(seat) {
            Some(runtime) => runtime.data_control_devices.clone(),
            None => Vec::new(),
        };
        for dev in devices {
            if dev.is_null() {
                continue;
            }
            advertise_current_selection(state, dev, seat);
        }
    }
}

/// The seat runtime is going away: post `finished` on every device so
/// well-behaved managers release their objects, then drop our registrations.
pub(crate) unsafe fn finish_data_control_devices(state: *mut State, seat: SeatId) {
    unsafe {
        let devices: Vec<*mut ffi::wl_resource> = match (*state).seat_runtime(seat) {
            Some(runtime) => runtime.data_control_devices.clone(),
            None => Vec::new(),
        };
        for dev in devices {
            if dev.is_null() {
                continue;
            }
            ffi::wl_resource_post_event(dev, ffi::EXT_DATA_CONTROL_DEVICE_V1_FINISHED);
            let rec = ffi::wl_resource_get_user_data(dev) as *mut DataControlDeviceRec;
            if !rec.is_null() {
                (*rec).seat = None;
            }
        }
    }
}
