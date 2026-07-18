use super::*;

impl CompositorRuntime {
    fn wait_for_work(&mut self) -> bool {
        // Animation frames use a bounded wait so input can wake the compositor
        // immediately. Idle frames block until input, a client commit, or the
        // one-second maintenance tick arrives.
        let alive = if self.animating {
            let frame_interval = std::time::Duration::from_micros(16_667);
            let remaining = frame_interval.saturating_sub(self.previous_frame_at.elapsed());
            if remaining.is_zero() {
                self.host.dispatch_nonblocking()
            } else {
                self.host.dispatch_timeout(remaining)
            }
        } else {
            self.host
                .dispatch_timeout(std::time::Duration::from_secs(1))
        };
        alive && !self.shell.should_quit() && !self.quit_requested
    }

    fn update_animation_state(&mut self) {
        self.animating = self.shell.anim_pending()
            || self.server.transitions_pending()
            || self.capture_worker.is_busy()
            || self
                .wallpaper
                .as_ref()
                .is_some_and(ass_wallpaper::Wallpaper::has_model);
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
            "ass: {} session ended after {} frames",
            self.host.name(),
            self.frame_count
        );
        self.device.wait_idle();
        Ok(())
    }
}
