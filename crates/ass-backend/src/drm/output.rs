use super::*;

impl DrmBackend {
    pub(super) fn reconfigure_outputs(&mut self) {
        self.hotplug_pending = false;
        let selected = match select_outputs(self.card(), &self.configured_modes) {
            Ok(displays) => displays,
            Err(DrmError::NoConnector) => {
                log::info!("drm: all outputs disconnected; suspending rendering");
                if self.modeset_done {
                    let _ = self.disable_outputs();
                }
                self.modeset_done = false;
                self.pending_flips.clear();
                if let Some(scanout) = self.retiring.take() {
                    self.release_scanout(scanout);
                }
                if let Some(scanout) = self.current.take() {
                    self.release_scanout(scanout);
                }
                self.render_ready = false;
                return;
            }
            Err(error) => {
                log::warn!("drm: hotplug reprobe failed; keeping current layout: {error}");
                return;
            }
        };

        if display_signature(&selected) == display_signature(&self.displays) {
            // Probe created fresh mode blobs. The existing display set remains
            // authoritative, so release only the redundant probe resources.
            for output in selected.outputs {
                let _ = self.card().destroy_property_blob(output.mode_blob_id);
            }
            self.render_ready = true;
            return;
        }

        if self.modeset_done
            && let Err(error) = self.disable_outputs()
        {
            log::warn!("drm: failed to disable old hotplug layout: {error}");
        }
        self.modeset_done = false;
        self.pending_flips.clear();
        if let Some(scanout) = self.retiring.take() {
            self.release_scanout(scanout);
        }
        if let Some(scanout) = self.current.take() {
            self.release_scanout(scanout);
        }

        let old = std::mem::replace(&mut self.displays, selected);
        for output in old.outputs {
            let _ = self.card().destroy_property_blob(output.mode_blob_id);
        }
        if self.displays.modifiers != self.surface_modifiers {
            // The live Flux surface was created with the old intersection and
            // resize cannot retcon its modifier; the main loop must recreate
            // it (see Backend::surface_needs_recreate).
            log::info!(
                "drm: modifier intersection changed; presentation surface must be recreated"
            );
            self.surface_stale = true;
        }
        let (width, height) = self.displays.size;
        self.pointer.0 = self.pointer.0.clamp(0.0, width.saturating_sub(1) as f32);
        self.pointer.1 = self.pointer.1.clamp(0.0, height.saturating_sub(1) as f32);
        self.explicit_sync = self.sync_capable
            && self
                .displays
                .outputs
                .iter()
                .all(|output| output.props.plane_in_fence_fd.is_some());
        self.pending_resize = Some(Size {
            w: width as i32,
            h: height as i32,
        });
        self.render_ready = true;
        log::info!(
            "drm: hotplug layout now has {} output(s), desktop {}x{}",
            self.displays.outputs.len(),
            width,
            height
        );
    }

    pub(super) fn release_scanout(&self, scanout: Scanout) {
        let card = self.card();
        if let Err(error) = card.destroy_framebuffer(scanout.framebuffer) {
            log::warn!("DRM: failed to destroy framebuffer: {error}");
        }
        if let Err(error) = card.close_buffer(scanout.gem) {
            log::warn!("DRM: failed to close imported GEM handle: {error}");
        }
    }

    pub(super) fn disable_outputs(&self) -> Result<(), DrmError> {
        let mut request = atomic::AtomicModeReq::new();
        for output in &self.displays.outputs {
            let props = output.props;
            request.add_property(
                output.plane,
                props.plane_fb_id,
                property::Value::Framebuffer(None),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_id,
                property::Value::CRTC(None),
            );
            request.add_property(
                output.connector,
                props.connector_crtc_id,
                property::Value::CRTC(None),
            );
            request.add_property(
                output.crtc,
                props.crtc_active,
                property::Value::Boolean(false),
            );
        }
        self.card()
            .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, request)?;
        Ok(())
    }
}

pub(super) fn candidate_cards() -> Vec<PathBuf> {
    candidate_cards_with_override(std::env::var_os("ASS_DRM_DEVICE"))
}

pub(super) fn candidate_cards_with_override(
    override_path: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    if let Some(path) = override_path {
        return vec![PathBuf::from(path)];
    }
    (0..16)
        .map(|index| PathBuf::from(format!("/dev/dri/card{index}")))
        .filter(|path| path.exists())
        .collect()
}

/// Milliseconds until `deadline`, shaped as a `poll(2)` timeout: `None`
/// blocks indefinitely and an already-passed deadline polls without blocking.
pub(super) fn poll_ms_remaining(deadline: Option<std::time::Instant>) -> i32 {
    match deadline {
        None => -1,
        Some(deadline) => deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(i32::MAX as u128) as i32,
    }
}

/// Whether an optional pump deadline has been reached. `None` never expires.
pub(super) fn deadline_passed(deadline: Option<std::time::Instant>) -> bool {
    deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
}

pub(super) fn open_card_and_outputs(
    seat: &Rc<RefCell<libseat::Seat>>,
    configured_modes: &HashMap<String, ModeSpec>,
) -> Result<(Card, DisplaySet), DrmError> {
    let candidates = candidate_cards();
    let tried = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for path in candidates {
        match seat.borrow_mut().open_device(&path) {
            Ok(device) => {
                let card = Card { device, path };
                let result = card
                    .set_client_capability(drm::ClientCapability::UniversalPlanes, true)
                    .and_then(|()| card.set_client_capability(drm::ClientCapability::Atomic, true))
                    .map_err(DrmError::from)
                    .and_then(|()| select_outputs(&card, configured_modes));
                match result {
                    Ok(output) => return Ok((card, output)),
                    Err(error) => {
                        log::warn!(
                            "drm: skipping unusable card {}: {error}",
                            card.path.display()
                        );
                        if let Err(close_error) = seat.borrow_mut().close_device(card.device) {
                            log::warn!(
                                "libseat: failed to close skipped card {}: {close_error:?}",
                                card.path.display()
                            );
                        }
                    }
                }
            }
            Err(error) => log::warn!("libseat: cannot open {}: {error:?}", path.display()),
        }
    }
    Err(DrmError::NoCard(if tried.is_empty() {
        "/dev/dri/card[0-15] (none exist)".to_owned()
    } else {
        tried
    }))
}

#[derive(Debug, Clone)]
pub(super) struct OutputCandidate {
    pub(super) connector: connector::Handle,
    pub(super) name: String,
    pub(super) mode: Mode,
    pub(super) choices: Vec<OutputChoice>,
    pub(super) available_modes: Vec<OutputMode>,
}

#[derive(Debug, Clone)]
pub(super) struct OutputChoice {
    pub(super) crtc: crtc::Handle,
    pub(super) plane: plane::Handle,
    pub(super) modifiers: Vec<u64>,
}

pub(super) fn select_outputs(
    card: &Card,
    configured_modes: &HashMap<String, ModeSpec>,
) -> Result<DisplaySet, DrmError> {
    let resources = card.resource_handles()?;
    let mut connectors = resources
        .connectors()
        .iter()
        .filter_map(|handle| card.get_connector(*handle, true).ok())
        .filter(|info| info.state() == connector::State::Connected && !info.modes().is_empty())
        .collect::<Vec<_>>();
    connectors.sort_by_key(|info| (info.interface() as u32, info.interface_id()));
    if connectors.is_empty() {
        return Err(DrmError::NoConnector);
    }

    let planes = card
        .plane_handles()?
        .into_iter()
        .filter_map(|handle| card.get_plane(handle).ok().map(|info| (handle, info)))
        .filter(|(handle, _)| plane_type(card, *handle) == Some(control::PlaneType::Primary))
        .collect::<Vec<_>>();

    let mut assignment = None;
    for format in [DrmFourcc::Xrgb8888, DrmFourcc::Argb8888] {
        let mut candidates = Vec::with_capacity(connectors.len());
        for connector in &connectors {
            let name = connector.to_string();
            // (width, height, refresh_mhz, preferred) in connector order, so
            // an index returned by pick_mode addresses connector.modes()
            // directly.
            let tuples = connector
                .modes()
                .iter()
                .map(|mode| {
                    let (width, height) = mode.size();
                    (
                        width as i32,
                        height as i32,
                        mode.vrefresh().saturating_mul(1_000),
                        mode.mode_type().contains(ModeTypeFlags::PREFERRED),
                    )
                })
                .collect::<Vec<_>>();
            let spec = configured_modes.get(&name);
            let picked = match pick_mode(&tuples, spec) {
                Some(index) => index,
                None => {
                    // Only reachable with a spec that matched nothing.
                    if let Some(spec) = spec {
                        log::warn!(
                            "drm: {name}: configured mode {spec:?} matches no advertised mode; using the preferred mode"
                        );
                    }
                    pick_mode(&tuples, None).unwrap_or(0)
                }
            };
            let mode = connector.modes()[picked];
            let mut crtcs = Vec::new();
            if let Some(current) = connector
                .current_encoder()
                .and_then(|encoder| card.get_encoder(encoder).ok())
                .and_then(|encoder| encoder.crtc())
            {
                crtcs.push(current);
            }
            for encoder in connector.encoders() {
                if let Ok(encoder) = card.get_encoder(*encoder) {
                    for crtc in resources.filter_crtcs(encoder.possible_crtcs()) {
                        if !crtcs.contains(&crtc) {
                            crtcs.push(crtc);
                        }
                    }
                }
            }

            let mut choices = Vec::new();
            for crtc in crtcs {
                for (plane, info) in &planes {
                    if !resources
                        .filter_crtcs(info.possible_crtcs())
                        .contains(&crtc)
                        || !info.formats().contains(&(format as u32))
                    {
                        continue;
                    }
                    let modifiers = plane_modifiers(card, *plane, format)?;
                    if !modifiers.is_empty() {
                        choices.push(OutputChoice {
                            crtc,
                            plane: *plane,
                            modifiers,
                        });
                    }
                }
            }
            candidates.push(OutputCandidate {
                connector: connector.handle(),
                name,
                mode,
                choices,
                available_modes: advertised_modes(connector),
            });
        }
        if candidates
            .iter()
            .any(|candidate| candidate.choices.is_empty())
        {
            continue;
        }
        if let Some((choices, modifiers)) = assign_outputs(&candidates) {
            assignment = Some((format, candidates, choices, modifiers));
            break;
        }
    }

    let Some((format, candidates, choices, modifiers)) = assignment else {
        return Err(DrmError::NoPlane);
    };
    let mut desktop_width = 0_u32;
    let mut desktop_height = 0_u32;
    for candidate in &candidates {
        let size = candidate.mode.size();
        desktop_width = desktop_width
            .checked_add(size.0 as u32)
            .ok_or(DrmError::DesktopTooLarge(u32::MAX, desktop_height))?;
        desktop_height = desktop_height.max(size.1 as u32);
    }
    if !resources.supported_fb_width().contains(&desktop_width)
        || !resources.supported_fb_height().contains(&desktop_height)
    {
        return Err(DrmError::DesktopTooLarge(desktop_width, desktop_height));
    }

    let mut outputs: Vec<Output> = Vec::with_capacity(candidates.len());
    let mut x = 0_u32;
    for (candidate, choice) in candidates.into_iter().zip(choices) {
        let result = build_output(card, candidate, choice, x);
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                for output in &outputs {
                    let _ = card.destroy_property_blob(output.mode_blob_id);
                }
                return Err(error);
            }
        };
        let size = output.mode.size();
        x += size.0 as u32;
        outputs.push(output);
    }
    Ok(DisplaySet {
        outputs,
        size: (desktop_width, desktop_height),
        format,
        modifiers,
    })
}

pub(super) fn assign_outputs(
    candidates: &[OutputCandidate],
) -> Option<(Vec<OutputChoice>, Vec<u64>)> {
    pub(super) fn recurse(
        candidates: &[OutputCandidate],
        index: usize,
        used_crtcs: &mut HashSet<crtc::Handle>,
        used_planes: &mut HashSet<plane::Handle>,
        selected: &mut Vec<OutputChoice>,
        shared: Option<Vec<u64>>,
    ) -> Option<(Vec<OutputChoice>, Vec<u64>)> {
        if index == candidates.len() {
            return Some((selected.clone(), shared.unwrap_or_default()));
        }
        for choice in &candidates[index].choices {
            if used_crtcs.contains(&choice.crtc) || used_planes.contains(&choice.plane) {
                continue;
            }
            let next_shared = match &shared {
                Some(current) => current
                    .iter()
                    .copied()
                    .filter(|modifier| choice.modifiers.contains(modifier))
                    .collect::<Vec<_>>(),
                None => choice.modifiers.clone(),
            };
            if next_shared.is_empty() {
                continue;
            }
            used_crtcs.insert(choice.crtc);
            used_planes.insert(choice.plane);
            selected.push(choice.clone());
            if let Some(result) = recurse(
                candidates,
                index + 1,
                used_crtcs,
                used_planes,
                selected,
                Some(next_shared),
            ) {
                return Some(result);
            }
            selected.pop();
            used_crtcs.remove(&choice.crtc);
            used_planes.remove(&choice.plane);
        }
        None
    }

    recurse(
        candidates,
        0,
        &mut HashSet::new(),
        &mut HashSet::new(),
        &mut Vec::new(),
        None,
    )
}

pub(super) fn build_output(
    card: &Card,
    candidate: OutputCandidate,
    choice: OutputChoice,
    x: u32,
) -> Result<Output, DrmError> {
    let connector_props = property_map(card, candidate.connector)?;
    let crtc_props = property_map(card, choice.crtc)?;
    let plane_props = property_map(card, choice.plane)?;
    let props = AtomicProperties {
        connector_crtc_id: required_prop(&connector_props, "CRTC_ID")?,
        crtc_mode_id: required_prop(&crtc_props, "MODE_ID")?,
        crtc_active: required_prop(&crtc_props, "ACTIVE")?,
        plane_fb_id: required_prop(&plane_props, "FB_ID")?,
        plane_crtc_id: required_prop(&plane_props, "CRTC_ID")?,
        plane_src_x: required_prop(&plane_props, "SRC_X")?,
        plane_src_y: required_prop(&plane_props, "SRC_Y")?,
        plane_src_w: required_prop(&plane_props, "SRC_W")?,
        plane_src_h: required_prop(&plane_props, "SRC_H")?,
        plane_crtc_x: required_prop(&plane_props, "CRTC_X")?,
        plane_crtc_y: required_prop(&plane_props, "CRTC_Y")?,
        plane_crtc_w: required_prop(&plane_props, "CRTC_W")?,
        plane_crtc_h: required_prop(&plane_props, "CRTC_H")?,
        plane_in_fence_fd: optional_prop(&plane_props, "IN_FENCE_FD"),
    };
    let mode_blob = card.create_property_blob(&candidate.mode)?;
    let property::Value::Blob(mode_blob_id) = mode_blob else {
        unreachable!("create_property_blob always returns Blob")
    };
    Ok(Output {
        connector: candidate.connector,
        name: candidate.name,
        crtc: choice.crtc,
        plane: choice.plane,
        mode: candidate.mode,
        mode_blob,
        mode_blob_id,
        x,
        y: 0,
        props,
        available_modes: candidate.available_modes,
    })
}

/// The connector's advertised modes, deduplicated by (width, height,
/// refresh) and sorted by pixel count then refresh rate, highest first — the
/// order `ass-control outputs` presents them in.
pub(super) fn advertised_modes(info: &connector::Info) -> Vec<OutputMode> {
    let mut modes: Vec<OutputMode> = info
        .modes()
        .iter()
        .map(|mode| {
            let (width, height) = mode.size();
            OutputMode {
                width: width as i32,
                height: height as i32,
                refresh_mhz: mode.vrefresh().saturating_mul(1_000),
            }
        })
        .collect();
    modes.sort_by(|a, b| {
        (i64::from(b.width) * i64::from(b.height), b.refresh_mhz)
            .cmp(&(i64::from(a.width) * i64::from(a.height), a.refresh_mhz))
    });
    modes.dedup();
    modes
}

/// Choose a mode index out of `modes` — `(width, height, refresh_mhz,
/// preferred)` tuples in connector order — honoring an optional configured
/// spec (ADR-0028). Without a spec the DRM PREFERRED mode wins, falling back
/// to the first advertised mode. With a spec, matches require exact
/// width/height (plus a whole-Hz refresh match when the spec names one);
/// among matches the PREFERRED flag wins, then the highest refresh, then the
/// lowest index. `None` means nothing matched (or `modes` is empty); the
/// caller falls back to the no-spec rule.
pub(super) fn pick_mode(modes: &[(i32, i32, u32, bool)], spec: Option<&ModeSpec>) -> Option<usize> {
    match spec {
        None => modes
            .iter()
            .position(|&(.., preferred)| preferred)
            .or((!modes.is_empty()).then_some(0)),
        Some(spec) => modes
            .iter()
            .enumerate()
            .filter(|&(_, &(width, height, refresh_mhz, _))| {
                spec.matches(&OutputMode {
                    width,
                    height,
                    refresh_mhz,
                })
            })
            .max_by_key(|&(index, &(.., refresh_mhz, preferred))| {
                (preferred, refresh_mhz, std::cmp::Reverse(index))
            })
            .map(|(index, _)| index),
    }
}

pub(super) fn display_signature(displays: &DisplaySet) -> DisplaySignature {
    (
        displays.format,
        displays.modifiers.clone(),
        displays
            .outputs
            .iter()
            .map(|output| {
                let (width, height) = output.mode.size();
                (
                    output.name.clone(),
                    width as u32,
                    height as u32,
                    output.mode.vrefresh(),
                    output.x,
                    output.y,
                )
            })
            .collect(),
    )
}

pub(super) fn property_map<H: ResourceHandle>(
    card: &Card,
    handle: H,
) -> Result<HashMap<String, property::Info>, DrmError> {
    Ok(card.get_properties(handle)?.as_hashmap(card)?)
}

pub(super) fn required_prop(
    props: &HashMap<String, property::Info>,
    name: &'static str,
) -> Result<property::Handle, DrmError> {
    props
        .get(name)
        .map(property::Info::handle)
        .ok_or(DrmError::MissingProperty(name))
}

pub(super) fn optional_prop(
    props: &HashMap<String, property::Info>,
    name: &str,
) -> Option<property::Handle> {
    props.get(name).map(property::Info::handle)
}

pub(super) fn plane_type(card: &Card, handle: plane::Handle) -> Option<control::PlaneType> {
    let properties = card.get_properties(handle).ok()?;
    for (&id, &value) in properties.iter() {
        let info = card.get_property(id).ok()?;
        if info.name() == c"type" {
            return match value as u32 {
                value if value == control::PlaneType::Primary as u32 => {
                    Some(control::PlaneType::Primary)
                }
                value if value == control::PlaneType::Cursor as u32 => {
                    Some(control::PlaneType::Cursor)
                }
                value if value == control::PlaneType::Overlay as u32 => {
                    Some(control::PlaneType::Overlay)
                }
                _ => None,
            };
        }
    }
    None
}

/// Return modifiers accepted by `plane` for `format`. Drivers predating the
/// IN_FORMATS property expose only the legacy implicit-layout contract, whose
/// portable dma-buf representation is linear.
pub(super) fn plane_modifiers(
    card: &Card,
    plane: plane::Handle,
    format: DrmFourcc,
) -> Result<Vec<u64>, DrmError> {
    let properties = card.get_properties(plane)?;
    for (&id, &value) in properties.iter() {
        let info = card.get_property(id)?;
        if info.name().to_bytes() == b"IN_FORMATS" {
            if value == 0 {
                return Ok(vec![u64::from(DrmModifier::Linear)]);
            }
            let blob = card.get_property_blob(value)?;
            return parse_format_modifiers(&blob, format as u32);
        }
    }
    Ok(vec![u64::from(DrmModifier::Linear)])
}

/// Parse Linux's `drm_format_modifier_blob` without casting untrusted kernel
/// offsets to native structs. All bounds and arithmetic are checked first.
pub(super) fn parse_format_modifiers(blob: &[u8], format: u32) -> Result<Vec<u64>, DrmError> {
    const HEADER: usize = 24;
    const MODIFIER_RECORD: usize = 24;
    if blob.len() < HEADER {
        return Err(DrmError::MalformedFormats("short header"));
    }
    let read_u32 = |offset: usize| -> Result<u32, DrmError> {
        let bytes = blob
            .get(offset..offset + 4)
            .ok_or(DrmError::MalformedFormats("u32 outside blob"))?;
        Ok(u32::from_ne_bytes(bytes.try_into().unwrap()))
    };
    let read_u64 = |offset: usize| -> Result<u64, DrmError> {
        let bytes = blob
            .get(offset..offset + 8)
            .ok_or(DrmError::MalformedFormats("u64 outside blob"))?;
        Ok(u64::from_ne_bytes(bytes.try_into().unwrap()))
    };

    let count_formats = read_u32(8)? as usize;
    let formats_offset = read_u32(12)? as usize;
    let count_modifiers = read_u32(16)? as usize;
    let modifiers_offset = read_u32(20)? as usize;
    let formats_bytes = count_formats
        .checked_mul(4)
        .and_then(|size| formats_offset.checked_add(size))
        .ok_or(DrmError::MalformedFormats("format array overflow"))?;
    let modifiers_bytes = count_modifiers
        .checked_mul(MODIFIER_RECORD)
        .and_then(|size| modifiers_offset.checked_add(size))
        .ok_or(DrmError::MalformedFormats("modifier array overflow"))?;
    if formats_offset < HEADER || formats_bytes > blob.len() {
        return Err(DrmError::MalformedFormats("format array outside blob"));
    }
    if modifiers_offset < HEADER || modifiers_bytes > blob.len() {
        return Err(DrmError::MalformedFormats("modifier array outside blob"));
    }

    let Some(format_index) =
        (0..count_formats).find(|index| read_u32(formats_offset + index * 4).ok() == Some(format))
    else {
        return Ok(Vec::new());
    };
    let mut modifiers = Vec::new();
    for index in 0..count_modifiers {
        let base = modifiers_offset + index * MODIFIER_RECORD;
        let formats = read_u64(base)?;
        let offset = read_u32(base + 8)? as usize;
        if format_index >= offset
            && format_index - offset < 64
            && formats & (1_u64 << (format_index - offset)) != 0
        {
            let modifier = read_u64(base + 16)?;
            if modifier != u64::from(DrmModifier::Invalid) && !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
        }
    }
    // Prefer linear when both sides support it. Flux applies the same policy,
    // but ordering here also makes logs/tests deterministic.
    modifiers.sort_by_key(|modifier| (*modifier != u64::from(DrmModifier::Linear), *modifier));
    Ok(modifiers)
}
