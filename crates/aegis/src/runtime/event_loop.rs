use super::*;

impl CompositorRuntime {
    fn wait_for_work(&mut self) -> bool {
        // Animation frames use a bounded wait so input can wake the compositor
        // immediately. Idle frames block until input, a client commit, or the
        // one-second maintenance tick arrives.
        let alive = if self.animating {
            let frame_interval = self.frame_interval();
            let remaining = frame_interval.saturating_sub(self.previous_frame_at.elapsed());
            if remaining.is_zero() {
                self.host.dispatch_nonblocking()
            } else {
                self.host.dispatch_timeout(remaining)
            }
        } else {
            // Animated wallpapers (video, multi-frame GIF/WebP) pace their own
            // frames: wake in time for the next one instead of leaving them
            // frozen on the maintenance tick, which still bounds the wait so
            // idle housekeeping keeps its one-second cadence.
            let wait = self
                .wallpaper
                .as_ref()
                .and_then(aegis_wallpaper::Wallpaper::next_frame_in)
                .unwrap_or(std::time::Duration::from_secs(1))
                .min(std::time::Duration::from_secs(1));
            self.host.dispatch_timeout(wait)
        };
        alive && !self.shell.should_quit() && !self.quit_requested
    }

    /// Frame pacing for the animation tick, derived from the fastest active
    /// output's refresh rate and capped at 60fps. Rendering a full-resolution
    /// animated wallpaper faster than that consumes substantial GPU bandwidth
    /// without improving shell responsiveness. Backends that report no mode
    /// (nested, where the outer compositor owns pacing) keep 60 Hz.
    fn frame_interval(&self) -> std::time::Duration {
        let refresh_mhz = self
            .server
            .output_infos()
            .iter()
            .map(|output| output.geometry.mode.refresh_mhz)
            .max()
            .unwrap_or(0);
        const SIXTY_HZ: std::time::Duration = std::time::Duration::from_micros(16_667);
        if refresh_mhz == 0 {
            SIXTY_HZ
        } else {
            // refresh_mhz is in millihertz: period_ns = 1e12 / refresh_mhz.
            std::time::Duration::from_nanos(1_000_000_000_000 / u64::from(refresh_mhz))
                .max(SIXTY_HZ)
        }
    }

    fn update_animation_state(&mut self) {
        self.animating = self.shell.anim_pending()
            || self.server.transitions_pending()
            || self.capture_worker.is_busy()
            || self
                .wallpaper
                .as_ref()
                .is_some_and(aegis_wallpaper::Wallpaper::has_model);
    }

    pub(super) fn run_loop(mut self) -> Result<(), Box<dyn std::error::Error>> {
        while self.wait_for_work() {
            let Some(work) = self.prepare_iteration()? else {
                continue;
            };
            let frame = self.process_input(work)?;
            if matches!(self.render_and_present(frame)?, PresentationOutcome::Retry) {
                continue;
            }
            self.update_animation_state();
        }

        log::info!(
            "aegis: {} session ended after {} frames",
            self.host.name(),
            self.frame_count
        );
        self.device.wait_idle();
        Ok(())
    }
}
