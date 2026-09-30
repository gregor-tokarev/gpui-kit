use gpui::{Context, Pixels, Task, px};
use instant::Duration;

static INTERVAL: Duration = Duration::from_millis(500);
static PAUSE_DELAY: Duration = Duration::from_millis(300);
/// Blinks before the cursor settles, visible, until the next input: ten seconds.
/// Each blink redraws the window, so an idle focused input would otherwise keep
/// the window drawing twice a second.
const BLINKS_BEFORE_SETTLING: usize = 20;

// On Windows, Linux, we should use integer to avoid blurry cursor.
#[cfg(not(target_os = "macos"))]
pub(super) const CURSOR_WIDTH: Pixels = px(2.);
#[cfg(target_os = "macos")]
pub(super) const CURSOR_WIDTH: Pixels = px(1.5);

/// To manage the Input cursor blinking.
///
/// It will start blinking with a interval of 500ms.
/// Every loop will notify the view to update the `visible`, and Input will observe this update to touch repaint.
/// After ten seconds without input, it stops blinking and keeps the cursor visible.
///
/// The input painter will check if this in visible state, then it will draw the cursor.
pub(crate) struct BlinkCursor {
    visible: bool,
    paused: bool,
    epoch: usize,
    blinks_left: usize,

    _task: Task<()>,
}

impl BlinkCursor {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            paused: false,
            epoch: 0,
            blinks_left: BLINKS_BEFORE_SETTLING,
            _task: Task::ready(()),
        }
    }

    /// Start the blinking
    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        self.blinks_left = BLINKS_BEFORE_SETTLING;
        self.blink(self.epoch, cx);
    }

    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        self.epoch = 0;
        cx.notify();
    }

    fn next_epoch(&mut self) -> usize {
        self.epoch += 1;
        self.epoch
    }

    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if self.paused || epoch != self.epoch {
            self.visible = true;
            return;
        }

        if self.blinks_left == 0 {
            if !self.visible {
                self.visible = true;
                cx.notify();
            }

            return;
        }

        self.blinks_left -= 1;
        self.visible = !self.visible;
        cx.notify();

        // Schedule the next blink
        let epoch = self.next_epoch();
        self._task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INTERVAL).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.blink(epoch, cx));
            }
        });
    }

    pub(crate) fn visible(&self) -> bool {
        // Keep showing the cursor if paused
        self.paused || self.visible
    }

    /// Show the cursor immediately and restart the idle delay before blinking resumes.
    pub(crate) fn pause(&mut self, cx: &mut Context<Self>) {
        self.paused = true;
        self.visible = true;
        cx.notify();

        // Every pause replaces the pending timer, keeping repeated input visible.
        let epoch = self.next_epoch();
        self._task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE_DELAY).await;

            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    this.paused = false;
                    this.blinks_left = BLINKS_BEFORE_SETTLING;
                    this.blink(epoch, cx);
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};
    use std::{cell::Cell, rc::Rc};

    #[gpui::test]
    fn repeated_pauses_keep_cursor_visible_until_idle(cx: &mut TestAppContext) {
        let cursor = cx.new(|_| BlinkCursor::new());
        assert!(!cursor.read_with(cx, |cursor, _| cursor.visible()));
        for _ in 0..5 {
            cursor.update(cx, |cursor, cx| cursor.pause(cx));
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.run_until_parked();
            assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));
        }
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert!(!cursor.read_with(cx, |cursor, _| cursor.visible()));
        cx.executor().advance_clock(INTERVAL);
        cx.run_until_parked();
        assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));
    }

    #[gpui::test]
    fn idle_cursor_settles_visible_until_input(cx: &mut TestAppContext) {
        let cursor = cx.new(|_| BlinkCursor::new());
        let notifications = Rc::new(Cell::new(0));
        let _subscription = cx.update(|cx| {
            let notifications = notifications.clone();
            cx.observe(&cursor, move |_, _| {
                notifications.set(notifications.get() + 1)
            })
        });

        cursor.update(cx, |cursor, cx| cursor.start(cx));
        cx.run_until_parked();
        for _ in 0..BLINKS_BEFORE_SETTLING {
            cx.executor().advance_clock(INTERVAL);
            cx.run_until_parked();
        }
        assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));

        // A settled cursor schedules no further redraws.
        let settled = notifications.get();
        cx.executor().advance_clock(INTERVAL * 10);
        cx.run_until_parked();
        assert_eq!(notifications.get(), settled);
        assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));

        // Input restarts blinking once the pause delay passes.
        cursor.update(cx, |cursor, cx| cursor.pause(cx));
        cx.executor().advance_clock(PAUSE_DELAY);
        cx.run_until_parked();
        assert!(!cursor.read_with(cx, |cursor, _| cursor.visible()));
        cx.executor().advance_clock(INTERVAL);
        cx.run_until_parked();
        assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));
    }
}
