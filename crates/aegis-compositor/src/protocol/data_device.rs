use crate::*;

// ----- wl_data_device_manager (clipboard + DnD v3) ------------------------
//
// A functional per-seat clipboard: `set_selection` records the source and
// advertises a `wl_data_offer` to the focused client's `wl_data_device`.
// Client-owned payloads transfer through the source's `send` event;
// compositor-owned payloads use the bounded writer in `server::clipboard`.
// Version 3 action negotiation is implemented for copy/move/ask drag
// operations.

static DDM_IMPL: ffi::wl_data_device_manager_interface_impl =
    ffi::wl_data_device_manager_interface_impl {
        create_data_source: ddm_create_data_source,
        get_data_device: ddm_get_data_device,
    };

static DATA_DEVICE_IMPL: ffi::wl_data_device_interface_impl = ffi::wl_data_device_interface_impl {
    start_drag: ddev_start_drag,
    set_selection: ddev_set_selection,
    // wl_data_device gained release in v2. The global is advertised at v3,
    // so the implementation table must include opcode 2; otherwise
    // libwayland dispatches through memory past the Rust table when clients
    // release a dynamically removed Interaction Domain seat's data device.
    release: res_destroy,
};

static DATA_SOURCE_IMPL: ffi::wl_data_source_interface_impl = ffi::wl_data_source_interface_impl {
    offer: data_source_offer,
    destroy: res_destroy,
    set_actions: data_source_set_actions,
};

static DATA_OFFER_IMPL: ffi::wl_data_offer_interface_impl = ffi::wl_data_offer_interface_impl {
    accept: data_offer_accept,
    receive: data_offer_receive,
    destroy: res_destroy,
    finish: data_offer_finish,
    set_actions: data_offer_set_actions,
};

pub(crate) unsafe extern "C" fn ddm_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::wl_data_device_manager_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &DDM_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

/// Create a `wl_data_source` whose MIME types/actions are collected before use.
unsafe extern "C" fn ddm_create_data_source(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let ver = ffi::wl_resource_get_version(mgr);
        let src = ffi::wl_resource_create(client, &ffi::wl_data_source_interface, ver, id);
        if src.is_null() {
            return;
        }
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        let rec = Box::into_raw(Box::new(DataSourceRec {
            state,
            mime_types: Vec::new(),
            actions: if ver >= 3 {
                ffi::WL_DATA_ACTION_NONE
            } else {
                ffi::WL_DATA_ACTION_COPY
            },
            actions_set: false,
            used_for_drag: false,
        }));
        ffi::wl_resource_set_implementation(
            src,
            &DATA_SOURCE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(data_source_resource_destroy),
        );
    }
}

unsafe extern "C" fn data_source_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut DataSourceRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        if !state.is_null() {
            let _guard = ActiveSeatGuard::for_resource(state, resource, false);
            if (*state)
                .selection
                .as_ref()
                .is_some_and(|selection| selection.source == resource)
            {
                (*state).selection = None;
                notify_selection_changed(state);
            }
            if (*state)
                .drag
                .as_ref()
                .is_some_and(|drag| drag.source == resource)
            {
                cancel_drag(state, false);
            }
            // Offers are client-owned and can outlive their source. Make their
            // back-pointer inert so a late receive cannot address freed memory.
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
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn data_source_offer(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut DataSourceRec;
        if !rec.is_null()
            && !mime_type.is_null()
            && let Ok(s) = CStr::from_ptr(mime_type).to_str()
            && !(*rec).mime_types.iter().any(|m| m == s)
        {
            (*rec).mime_types.push(s.to_string());
        }
    }
}

unsafe extern "C" fn data_source_set_actions(
    _client: *mut ffi::wl_client,
    source: *mut ffi::wl_resource,
    actions: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
        if rec.is_null() {
            return;
        }
        if actions & !ffi::WL_DATA_ACTION_MASK != 0 {
            let msg = c"wl_data_source.set_actions: invalid action mask";
            ffi::wl_resource_post_error(
                source,
                ffi::WL_DATA_SOURCE_ERROR_INVALID_ACTION_MASK,
                msg.as_ptr(),
            );
            return;
        }
        if (*rec).actions_set || (*rec).used_for_drag {
            let msg = c"wl_data_source.set_actions may only be called once before start_drag";
            ffi::wl_resource_post_error(
                source,
                ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
                msg.as_ptr(),
            );
            return;
        }
        (*rec).actions = actions;
        (*rec).actions_set = true;
    }
}

unsafe extern "C" fn ddm_get_data_device(
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
        let seat_id = (*state).active_seat;
        let ver = ffi::wl_resource_get_version(mgr);
        let dev = ffi::wl_resource_create(client, &ffi::wl_data_device_interface, ver, id);
        if dev.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            dev,
            &DATA_DEVICE_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(data_device_resource_destroy),
        );
        (*state).track_routed_seat_resource(dev, advertised_seat, seat_id);
        (*state).data_devices.push(dev);
        // If a selection is already active, advertise it to this new device.
        if !(*state).keyboard_focus.is_null()
            && ffi::wl_resource_get_client((*state).keyboard_focus) == client
            && let Some(sel) = &(*state).selection
        {
            advertise_selection_offer(dev, sel);
        }
    }
}

unsafe extern "C" fn data_device_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(_guard) = ActiveSeatGuard::for_resource(state, resource, false) else {
            return;
        };
        if let Some(pos) = (*state).data_devices.iter().position(|p| *p == resource) {
            (*state).data_devices.remove(pos);
        }
        if let Some(drag) = (*state).drag.as_mut()
            && drag.target_device == resource
        {
            drag.focus = std::ptr::null_mut();
            drag.target_device = std::ptr::null_mut();
            drag.offer = std::ptr::null_mut();
        }
        (*state).untrack_seat_resource(resource);
    }
}

unsafe fn notify_selection_changed(state: *mut State) {
    unsafe {
        let focus_client = if (*state).keyboard_focus.is_null() {
            std::ptr::null_mut()
        } else {
            ffi::wl_resource_get_client((*state).keyboard_focus)
        };
        for dev in (*state).data_devices.clone() {
            if dev.is_null() {
                continue;
            }
            if !focus_client.is_null()
                && ffi::wl_resource_get_client(dev) == focus_client
                && let Some(selection) = &(*state).selection
            {
                advertise_selection_offer(dev, selection);
                continue;
            }
            ffi::wl_resource_post_event(
                dev,
                ffi::WL_DATA_DEVICE_SELECTION,
                std::ptr::null_mut::<ffi::wl_resource>(),
            );
        }
    }
}

pub(crate) unsafe fn replace_clipboard_selection(
    state: *mut State,
    replacement: Option<Selection>,
) {
    unsafe {
        let replacement_source = replacement
            .as_ref()
            .map(|selection| selection.source)
            .unwrap_or(std::ptr::null_mut());
        if let Some(old) = std::mem::replace(&mut (*state).selection, replacement)
            && !old.source.is_null()
            && old.source != replacement_source
        {
            ffi::wl_resource_post_event(old.source, ffi::WL_DATA_SOURCE_CANCELLED);
        }
        notify_selection_changed(state);
    }
}

/// `wl_data_device.set_selection`: record the source as the current selection
/// and advertise a `wl_data_offer` to every bound data device. A null source
/// clears the selection.
unsafe extern "C" fn ddev_set_selection(
    client: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    _serial: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(_r) as *mut State;
        let Some(_guard) = ActiveSeatGuard::for_resource(state, _r, true) else {
            return;
        };
        if (*state).keyboard_focus.is_null()
            || ffi::wl_resource_get_client((*state).keyboard_focus) != client
            || (!source.is_null() && ffi::wl_resource_get_client(source) != client)
        {
            return;
        }
        if source.is_null() {
            replace_clipboard_selection(state, None);
            return;
        }
        // Build the selection record: the source resource + its collected MIMEs.
        let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
        if !source_rec.is_null() && (*source_rec).actions_set {
            let msg = c"data source configured for drag-and-drop cannot own selection";
            ffi::wl_resource_post_error(
                source,
                ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
                msg.as_ptr(),
            );
            return;
        }
        let mimes = if source_rec.is_null() {
            Vec::new()
        } else {
            (*source_rec).mime_types.clone()
        };
        let sel = Selection {
            source,
            mime_types: mimes,
            owned: None,
        };
        (*state).track_seat_resource(source, (*state).active_seat);
        replace_clipboard_selection(state, Some(sel));
    }
}

pub(crate) unsafe fn data_device_focus_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    unsafe {
        if state.is_null() {
            return;
        }
        let old_client = if old_focus.is_null() {
            std::ptr::null_mut()
        } else {
            ffi::wl_resource_get_client(old_focus)
        };
        let new_client = if new_focus.is_null() {
            std::ptr::null_mut()
        } else {
            ffi::wl_resource_get_client(new_focus)
        };
        for device in (*state).data_devices.clone() {
            if device.is_null() {
                continue;
            }
            let client = ffi::wl_resource_get_client(device);
            if !old_client.is_null() && client == old_client {
                ffi::wl_resource_post_event(
                    device,
                    ffi::WL_DATA_DEVICE_SELECTION,
                    std::ptr::null_mut::<ffi::wl_resource>(),
                );
            }
            if !new_client.is_null() && client == new_client {
                if let Some(selection) = &(*state).selection {
                    advertise_selection_offer(device, selection);
                } else {
                    ffi::wl_resource_post_event(
                        device,
                        ffi::WL_DATA_DEVICE_SELECTION,
                        std::ptr::null_mut::<ffi::wl_resource>(),
                    );
                }
            }
        }
    }
}

/// Create a `wl_data_offer` for `sel`, send `data_offer` + its `offer` events
/// to `dev`, then `selection(offer)`.
unsafe fn create_data_offer(
    state: *mut State,
    dev: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    mime_types: &[String],
    owned: Option<OwnedSelection>,
    is_drag: bool,
) -> *mut ffi::wl_resource {
    unsafe {
        let client = ffi::wl_resource_get_client(dev);
        let version = ffi::wl_resource_get_version(dev).min(3);
        let offer = ffi::wl_resource_create(client, &ffi::wl_data_offer_interface, version, 0);
        if offer.is_null() {
            return std::ptr::null_mut();
        }
        let rec = Box::into_raw(Box::new(DataOfferRec {
            state,
            source,
            owned,
            is_drag,
            accepted: false,
            destination_actions: ffi::WL_DATA_ACTION_NONE,
            preferred_action: ffi::WL_DATA_ACTION_NONE,
            selected_action: if is_drag && version < 3 {
                ffi::WL_DATA_ACTION_COPY
            } else {
                ffi::WL_DATA_ACTION_NONE
            },
            dropped: false,
            finished: false,
        }));
        ffi::wl_resource_set_implementation(
            offer,
            &DATA_OFFER_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(data_offer_resource_destroy),
        );
        (*state).data_offers.push(offer);
        (*state).track_seat_resource(offer, (*state).active_seat);
        ffi::wl_resource_post_event(dev, ffi::WL_DATA_DEVICE_DATA_OFFER, offer);
        for mime in mime_types {
            let c = CString::new(mime.as_str()).unwrap();
            ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_OFFER, c.as_ptr());
        }
        if is_drag && version >= 3 {
            let actions = if source.is_null() {
                ffi::WL_DATA_ACTION_COPY
            } else {
                let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
                if source_rec.is_null() {
                    ffi::WL_DATA_ACTION_NONE
                } else {
                    (*source_rec).actions
                }
            };
            ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_SOURCE_ACTIONS, actions);
            ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_ACTION, ffi::WL_DATA_ACTION_NONE);
        }
        offer
    }
}

unsafe fn advertise_selection_offer(dev: *mut ffi::wl_resource, sel: &Selection) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(dev) as *mut State;
        let offer = create_data_offer(
            state,
            dev,
            sel.source,
            &sel.mime_types,
            sel.owned.clone(),
            false,
        );
        if offer.is_null() {
            return;
        }
        ffi::wl_resource_post_event(dev, ffi::WL_DATA_DEVICE_SELECTION, offer);
    }
}

unsafe extern "C" fn data_offer_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut DataOfferRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        let _guard = ActiveSeatGuard::for_resource(state, resource, false);
        let cancel_unfinished = (*rec).is_drag
            && (*rec).dropped
            && !(*rec).finished
            && !(*rec).source.is_null()
            && ffi::wl_resource_get_version((*rec).source) >= 3;
        if !state.is_null() {
            if let Some(pos) = (*state).data_offers.iter().position(|p| *p == resource) {
                (*state).data_offers.remove(pos);
            }
            (*state).untrack_seat_resource(resource);
            if let Some(drag) = (*state).drag.as_mut()
                && drag.offer == resource
            {
                drag.offer = std::ptr::null_mut();
            }
        }
        if cancel_unfinished {
            ffi::wl_resource_post_event((*rec).source, ffi::WL_DATA_SOURCE_CANCELLED);
        }
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn data_offer_accept(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    _serial: u32,
    mime_type: *const std::os::raw::c_char,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
        if rec.is_null() || !(*rec).is_drag || (*rec).source.is_null() {
            return;
        }
        (*rec).accepted = !mime_type.is_null();
        ffi::wl_resource_post_event((*rec).source, ffi::WL_DATA_SOURCE_TARGET, mime_type);
    }
}

pub(crate) fn choose_dnd_action(source: u32, destination: u32, preferred: u32) -> u32 {
    let available = source & destination & ffi::WL_DATA_ACTION_MASK;
    if preferred != 0 && available & preferred != 0 {
        preferred
    } else if available & ffi::WL_DATA_ACTION_COPY != 0 {
        ffi::WL_DATA_ACTION_COPY
    } else if available & ffi::WL_DATA_ACTION_MOVE != 0 {
        ffi::WL_DATA_ACTION_MOVE
    } else if available & ffi::WL_DATA_ACTION_ASK != 0 {
        ffi::WL_DATA_ACTION_ASK
    } else {
        ffi::WL_DATA_ACTION_NONE
    }
}

unsafe fn post_dnd_action(offer: *mut ffi::wl_resource, action: u32) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
        if rec.is_null() || (*rec).selected_action == action {
            return;
        }
        (*rec).selected_action = action;
        if ffi::wl_resource_get_version(offer) >= 3 {
            ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_ACTION, action);
        }
        let source = (*rec).source;
        if !source.is_null() && ffi::wl_resource_get_version(source) >= 3 {
            ffi::wl_resource_post_event(source, ffi::WL_DATA_SOURCE_ACTION, action);
        }
    }
}

unsafe extern "C" fn data_offer_set_actions(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    actions: u32,
    preferred: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
        if rec.is_null() || !(*rec).is_drag || (*rec).finished {
            ffi::wl_resource_post_error(
                offer,
                ffi::WL_DATA_OFFER_ERROR_INVALID_OFFER,
                c"set_actions is only valid on an active drag offer".as_ptr(),
            );
            return;
        }
        if actions & !ffi::WL_DATA_ACTION_MASK != 0 {
            ffi::wl_resource_post_error(
                offer,
                ffi::WL_DATA_OFFER_ERROR_INVALID_ACTION_MASK,
                c"wl_data_offer.set_actions: invalid action mask".as_ptr(),
            );
            return;
        }
        if preferred & !ffi::WL_DATA_ACTION_MASK != 0
            || preferred.count_ones() > 1
            || (preferred != 0 && actions & preferred == 0)
        {
            ffi::wl_resource_post_error(
                offer,
                ffi::WL_DATA_OFFER_ERROR_INVALID_ACTION,
                c"wl_data_offer.set_actions: invalid preferred action".as_ptr(),
            );
            return;
        }
        (*rec).destination_actions = actions;
        (*rec).preferred_action = preferred;
        let source_actions = if (*rec).source.is_null() {
            ffi::WL_DATA_ACTION_COPY
        } else {
            let source_rec = ffi::wl_resource_get_user_data((*rec).source) as *mut DataSourceRec;
            if source_rec.is_null() {
                ffi::WL_DATA_ACTION_NONE
            } else {
                (*source_rec).actions
            }
        };
        post_dnd_action(offer, choose_dnd_action(source_actions, actions, preferred));
    }
}

unsafe extern "C" fn data_offer_finish(_client: *mut ffi::wl_client, offer: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
        if rec.is_null()
            || !(*rec).is_drag
            || !(*rec).dropped
            || !(*rec).accepted
            || (*rec).selected_action == ffi::WL_DATA_ACTION_NONE
            || (*rec).finished
        {
            ffi::wl_resource_post_error(
                offer,
                ffi::WL_DATA_OFFER_ERROR_INVALID_FINISH,
                c"wl_data_offer.finish called before a successful drop".as_ptr(),
            );
            return;
        }
        (*rec).finished = true;
        if !(*rec).source.is_null() && ffi::wl_resource_get_version((*rec).source) >= 3 {
            ffi::wl_resource_post_event((*rec).source, ffi::WL_DATA_SOURCE_DND_FINISHED);
        }
    }
}

/// `wl_data_offer.receive`: forward to the source's `send` request so the
/// owning client writes the content for `mime_type` into `fd`.
unsafe extern "C" fn data_offer_receive(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
    fd: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
        if !rec.is_null()
            && let Some(owned) = &(*rec).owned
        {
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
        let source = if rec.is_null() {
            std::ptr::null_mut()
        } else {
            (*rec).source
        };
        if source.is_null() {
            if fd >= 0 {
                libc_close(fd);
            }
            return;
        }
        ffi::wl_resource_post_event(source, ffi::WL_DATA_SOURCE_SEND, mime_type, fd);
        // The source owns writing to fd; close our copy.
        if fd >= 0 {
            libc_close(fd);
        }
    }
}

unsafe extern "C" fn ddev_start_drag(
    client: *mut ffi::wl_client,
    data_device: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    origin: *mut ffi::wl_resource,
    icon: *mut ffi::wl_resource,
    serial: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(data_device) as *mut State;
        let Some(_guard) = ActiveSeatGuard::for_resource(state, data_device, true) else {
            return;
        };
        if !(*state).implicit_grab_active
            || serial != (*state).last_button_serial
            || origin.is_null()
            || ffi::wl_resource_get_client(origin) != client
            || origin != (*state).pointer_focus
            || (!source.is_null() && ffi::wl_resource_get_client(source) != client)
            || (!icon.is_null() && ffi::wl_resource_get_client(icon) != client)
        {
            return;
        }
        if !icon.is_null() {
            let icon_rec = ffi::wl_resource_get_user_data(icon) as *mut SurfaceRec;
            if icon_rec.is_null() || surface_has_role(&*icon_rec) {
                ffi::wl_resource_post_error(
                    data_device,
                    0,
                    c"drag icon surface already has a role".as_ptr(),
                );
                return;
            }
            (*icon_rec).drag_icon_role = true;
        }
        if !source.is_null() {
            if (*state)
                .selection
                .as_ref()
                .is_some_and(|selection| selection.source == source)
            {
                ffi::wl_resource_post_error(
                    source,
                    ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
                    c"selection source cannot be reused for drag-and-drop".as_ptr(),
                );
                return;
            }
            let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
            if source_rec.is_null() || (*source_rec).used_for_drag {
                ffi::wl_resource_post_error(
                    source,
                    ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
                    c"data source has already been used for drag-and-drop".as_ptr(),
                );
                return;
            }
            (*source_rec).used_for_drag = true;
            (*state).track_seat_resource(source, (*state).active_seat);
        }
        if (*state).drag.is_some() {
            cancel_drag(state, true);
        }
        (*state).drag = Some(DragState {
            source,
            origin,
            focus: std::ptr::null_mut(),
            target_device: std::ptr::null_mut(),
            offer: std::ptr::null_mut(),
            icon,
        });
        update_overlay_positions(state);
        update_drag_focus(
            state,
            (*state).pointer_focus,
            (*state).pointer_x,
            (*state).pointer_y,
            0,
        );
    }
}

unsafe fn data_device_for_client(
    state: *mut State,
    client: *mut ffi::wl_client,
) -> *mut ffi::wl_resource {
    unsafe {
        (*state)
            .data_devices
            .iter()
            .copied()
            .find(|device| !device.is_null() && ffi::wl_resource_get_client(*device) == client)
            .unwrap_or(std::ptr::null_mut())
    }
}

/// Move the active DnD implicit grab to `focus` and emit the version-1
/// data-device enter/leave/motion sequence. Coordinates are converted from
/// compositor logical space to the destination surface's local space.
pub(crate) unsafe fn update_drag_focus(
    state: *mut State,
    mut focus: *mut ffi::wl_resource,
    x: f32,
    y: f32,
    time: u32,
) {
    unsafe {
        let Some(mut drag) = (*state).drag else {
            return;
        };

        // A null source denotes client-internal DnD; the protocol restricts its
        // enter/motion events to surfaces belonging to the initiating client.
        if !focus.is_null()
            && drag.source.is_null()
            && ffi::wl_resource_get_client(focus) != ffi::wl_resource_get_client(drag.origin)
        {
            focus = std::ptr::null_mut();
        }
        let target_device = if focus.is_null() {
            std::ptr::null_mut()
        } else {
            data_device_for_client(state, ffi::wl_resource_get_client(focus))
        };
        if target_device.is_null() {
            focus = std::ptr::null_mut();
        }

        if focus == drag.focus && target_device == drag.target_device {
            if !target_device.is_null() {
                let surface = ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec;
                if !surface.is_null() {
                    let origin = surface_draw_origin(&*surface);
                    ffi::wl_resource_post_event(
                        target_device,
                        ffi::WL_DATA_DEVICE_MOTION,
                        time,
                        ffi::wl_fixed_from_f32(x - origin.x as f32),
                        ffi::wl_fixed_from_f32(y - origin.y as f32),
                    );
                }
            }
            return;
        }

        if !drag.target_device.is_null() {
            ffi::wl_resource_post_event(drag.target_device, ffi::WL_DATA_DEVICE_LEAVE);
        }
        if !drag.source.is_null() {
            ffi::wl_resource_post_event(
                drag.source,
                ffi::WL_DATA_SOURCE_TARGET,
                std::ptr::null::<std::os::raw::c_char>(),
            );
        }

        drag.focus = focus;
        drag.target_device = target_device;
        drag.offer = std::ptr::null_mut();

        if !target_device.is_null() {
            let mime_types = if drag.source.is_null() {
                Vec::new()
            } else {
                let source_rec = ffi::wl_resource_get_user_data(drag.source) as *mut DataSourceRec;
                if source_rec.is_null() {
                    Vec::new()
                } else {
                    (*source_rec).mime_types.clone()
                }
            };
            if !drag.source.is_null() {
                drag.offer =
                    create_data_offer(state, target_device, drag.source, &mime_types, None, true);
            }
            let surface = ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec;
            if !surface.is_null() {
                let serial = ffi::wl_display_next_serial((*state).display);
                let origin = surface_draw_origin(&*surface);
                ffi::wl_resource_post_event(
                    target_device,
                    ffi::WL_DATA_DEVICE_ENTER,
                    serial,
                    focus,
                    ffi::wl_fixed_from_f32(x - origin.x as f32),
                    ffi::wl_fixed_from_f32(y - origin.y as f32),
                    drag.offer,
                );
            }
        }
        (*state).drag = Some(drag);
    }
}

pub(crate) unsafe fn cancel_drag(state: *mut State, notify_source: bool) {
    unsafe {
        let Some(drag) = (*state).drag.take() else {
            return;
        };
        if !drag.target_device.is_null() {
            ffi::wl_resource_post_event(drag.target_device, ffi::WL_DATA_DEVICE_LEAVE);
        }
        if notify_source && !drag.source.is_null() {
            ffi::wl_resource_post_event(drag.source, ffi::WL_DATA_SOURCE_CANCELLED);
        }
        clear_drag_icon(drag.icon);
    }
}

unsafe fn clear_drag_icon(icon: *mut ffi::wl_resource) {
    unsafe {
        if icon.is_null() {
            return;
        }
        let rec = ffi::wl_resource_get_user_data(icon) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).drag_icon_role = false;
        }
    }
}

pub(crate) unsafe fn finish_drag(state: *mut State) {
    unsafe {
        let Some(drag) = (*state).drag.take() else {
            return;
        };
        let offer_rec = if drag.offer.is_null() {
            std::ptr::null_mut()
        } else {
            ffi::wl_resource_get_user_data(drag.offer) as *mut DataOfferRec
        };
        let accepted = !offer_rec.is_null()
            && (*offer_rec).accepted
            && (*offer_rec).selected_action != ffi::WL_DATA_ACTION_NONE;
        if drag.target_device.is_null() || !accepted {
            if !drag.source.is_null() {
                ffi::wl_resource_post_event(drag.source, ffi::WL_DATA_SOURCE_CANCELLED);
            }
        } else {
            (*offer_rec).dropped = true;
            if !drag.source.is_null() && ffi::wl_resource_get_version(drag.source) >= 3 {
                ffi::wl_resource_post_event(drag.source, ffi::WL_DATA_SOURCE_DND_DROP_PERFORMED);
            }
            ffi::wl_resource_post_event(drag.target_device, ffi::WL_DATA_DEVICE_DROP);
        }
        clear_drag_icon(drag.icon);
    }
}
