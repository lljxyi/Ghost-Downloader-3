//! 组件库（P4，第 71 步）：镜像 `app/view/components/band_selector.py`（136 行）
//! —— 任务列表的橡皮筋框选（框住的行区间 → 选中）。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - `_stride = rowHeight + rowSpacing`；`setItemCount` / `setEnabled(False)`
//!   都会在拖动中 `_cancelDrag()`。
//! - `eventFilter` 第一行就是 `if not self._isEnabled: return False` ——
//!   **禁用时所有事件都不处理**（连 release 都不清 `_isPending`）。
//! - 左键按下：只记 `_isPending=True` + 起点/末点/修饰键，**不消费事件**
//!   （返回 `False`，让列表自己处理选中）。
//! - 移动：仅 `pending or dragging` 时处理；先更新 `_lastX` 与
//!   `_lastViewportY = pos.y() - scrollBar.value()`；未拖动时用
//!   **曼哈顿距离** `< QApplication.startDragDistance()` 判断是否还算 pending。
//! - `_updateBand`：`first = max(0, top // stride)`、
//!   `last = min(itemCount - 1, bottom // stride)`，`first > last` 时发
//!   `bandChanged(-1, -1)`；框矩形宽高各自 `max(1, …)`。
//! - 自动滚动：`interval 30ms`、边距 30；上边界 `delta = max(-20, -(margin - viewportY) // 2)`
//!   ——注意 Python 里 `-(margin - viewportY) // 2` 是 `(-(margin - viewportY)) // 2`
//!   （先取负再**向下取整**），`viewportY = 29` 时得 `-1` 而不是 `0`；
//!   下边界 `delta = min(20, (viewportY - viewportHeight + margin) // 2)`；
//!   中间则 `stop()`。定时器已启动时不重复 `start()`。
//! - `_onAutoScroll`：`bar.setValue(bar.value() + delta)`，随后
//!   `_updateBand(lastX, lastViewportY + bar.value())`。
//! - `_onScrollChanged`：拖动中同样 `_updateBand(lastX, lastViewportY + value)`。
//! - `_cancelDrag`：清 dragging/pending、隐藏框、停表、发 `dragFinished`。
//!
//! ## 与 Qt 的差异
//!
//! - `startDragDistance` 与 `viewport().height()`、滚动条当前值都是外部实测
//!   值，本模块当入参/字段传入。
//! - 自动滚动时 Qt 会因 `setValue` 触发 `valueChanged` → `_onScrollChanged`
//!   → `_updateBand`，定时器随后再 `_updateBand` 一次（**两次 `bandChanged`**）。
//!   本模块只发一次（区间相同，幂等）；调用方若需要两次请自行重放。
//! - `stride <= 0` 时 Python 抛 `ZeroDivisionError`；这里按 `max(1)` 处理
//!   （行高恒正，属防御性偏差）。

/// `self._autoScrollTimer.setInterval(30)`
pub const AUTO_SCROLL_INTERVAL_MS: u64 = 30;
/// `margin = 30`
pub const AUTO_SCROLL_MARGIN: i32 = 30;
/// `max(-20, …)` / `min(20, …)`
pub const AUTO_SCROLL_MAX_DELTA: i32 = 20;

/// 框的几何（`QRubberBand.setGeometry(left, top, w, h)`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BandRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// `BandSelector` 的三个信号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandSignal {
    /// `dragStarted(Qt.KeyboardModifiers)`：载荷是修饰键位掩码。
    DragStarted(i32),
    /// `bandChanged(first, last)`；`(-1, -1)` 表示空区间。
    BandChanged(i32, i32),
    /// `dragFinished()`
    DragFinished,
}

/// 框选状态机（`BandSelector` 的可测部分）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BandSelector {
    stride: i32,
    item_count: i32,
    enabled: bool,
    dragging: bool,
    pending: bool,
    press_pos: (i32, i32),
    start_x: i32,
    start_y: i32,
    last_x: i32,
    last_viewport_y: i32,
    modifiers: i32,
    scroll_delta: i32,
    auto_scroll_active: bool,
    band: BandRect,
    band_visible: bool,
    scroll_value: i32,
    start_drag_distance: i32,
    signals: Vec<BandSignal>,
}

impl BandSelector {
    /// `BandSelector(scrollArea, scrollWidget, rowHeight, rowSpacing)`；
    /// `start_drag_distance` = `QApplication.startDragDistance()`。
    pub fn new(row_height: i32, row_spacing: i32, start_drag_distance: i32) -> Self {
        Self {
            stride: row_height + row_spacing,
            item_count: 0,
            enabled: true,
            dragging: false,
            pending: false,
            press_pos: (0, 0),
            start_x: 0,
            start_y: 0,
            last_x: 0,
            last_viewport_y: 0,
            modifiers: 0,
            scroll_delta: 0,
            auto_scroll_active: false,
            band: BandRect::default(),
            band_visible: false,
            scroll_value: 0,
            start_drag_distance,
            signals: Vec::new(),
        }
    }

    pub fn stride(&self) -> i32 {
        self.stride
    }

    pub fn item_count(&self) -> i32 {
        self.item_count
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    pub fn band(&self) -> BandRect {
        self.band
    }

    pub fn band_visible(&self) -> bool {
        self.band_visible
    }

    pub fn scroll_delta(&self) -> i32 {
        self.scroll_delta
    }

    pub fn auto_scroll_active(&self) -> bool {
        self.auto_scroll_active
    }

    pub fn scroll_value(&self) -> i32 {
        self.scroll_value
    }

    /// 取出并清空待发信号（顺序即源码里的 emit 顺序）。
    pub fn take_signals(&mut self) -> Vec<BandSignal> {
        std::mem::take(&mut self.signals)
    }

    /// 只看不取。
    pub fn signals(&self) -> &[BandSignal] {
        &self.signals
    }

    /// `setItemCount(count)`：拖动中直接取消。
    pub fn set_item_count(&mut self, count: i32) {
        self.item_count = count;
        if self.dragging {
            self.cancel_drag();
        }
    }

    /// `setEnabled(enabled)`：禁用时取消正在进行的拖动。
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled && self.dragging {
            self.cancel_drag();
        }
    }

    /// `WindowDeactivate`：拖动中取消；事件本身不消费（返回 `false`）。
    pub fn window_deactivate(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.dragging {
            self.cancel_drag();
        }
        false
    }

    /// 左键按下；返回是否消费事件（源码恒 `False`）。
    pub fn mouse_press(&mut self, x: i32, y: i32, modifiers: i32) -> bool {
        if !self.enabled {
            return false;
        }
        self.pending = true;
        self.press_pos = (x, y);
        self.start_x = x;
        self.start_y = y;
        self.last_x = x;
        self.last_viewport_y = y - self.scroll_value;
        self.modifiers = modifiers;
        false
    }

    /// 鼠标移动；返回是否消费事件。
    pub fn mouse_move(&mut self, x: i32, y: i32, viewport_height: i32) -> bool {
        if !self.enabled {
            return false;
        }
        if !(self.pending || self.dragging) {
            return false;
        }
        self.last_x = x;
        self.last_viewport_y = y - self.scroll_value;

        if !self.dragging {
            let dx = (x - self.press_pos.0).abs();
            let dy = (y - self.press_pos.1).abs();
            if dx + dy < self.start_drag_distance {
                return false;
            }
            self.dragging = true;
            self.pending = false;
            self.band_visible = true;
            self.signals.push(BandSignal::DragStarted(self.modifiers));
        }

        self.update_band(self.last_x, y, viewport_height);
        true
    }

    /// 左键松开；返回是否消费事件（拖动中取消并消费）。
    pub fn mouse_release(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.dragging {
            self.cancel_drag();
            return true;
        }
        self.pending = false;
        false
    }

    /// 滚动条 `valueChanged`：拖动中按新值重算框。
    pub fn on_scroll_changed(&mut self, value: i32, viewport_height: i32) {
        self.scroll_value = value;
        if self.dragging {
            self.update_band(self.last_x, self.last_viewport_y + value, viewport_height);
        }
    }

    /// `_onAutoScroll`：滚动一档后重算框（`max_value` = 滚动条最大值）。
    pub fn on_auto_scroll(&mut self, viewport_height: i32, max_value: i32) {
        if !self.dragging {
            return;
        }
        self.scroll_value = (self.scroll_value + self.scroll_delta).clamp(0, max_value);
        let y = self.last_viewport_y + self.scroll_value;
        self.update_band(self.last_x, y, viewport_height);
    }

    /// `_cancelDrag`
    pub fn cancel_drag(&mut self) {
        self.dragging = false;
        self.pending = false;
        self.band_visible = false;
        self.auto_scroll_active = false;
        self.signals.push(BandSignal::DragFinished);
    }

    /// `_updateBand(scrollWidgetX, scrollWidgetY)`
    fn update_band(&mut self, x: i32, y: i32, viewport_height: i32) {
        let left = self.start_x.min(x);
        let top = self.start_y.min(y);
        let right = self.start_x.max(x);
        let bottom = self.start_y.max(y);

        self.band = BandRect {
            left,
            top,
            width: 1.max(right - left),
            height: 1.max(bottom - top),
        };

        let stride = self.stride.max(1);
        let first = 0.max(top.div_euclid(stride));
        let last = (self.item_count - 1).min(bottom.div_euclid(stride));
        self.signals.push(if first <= last {
            BandSignal::BandChanged(first, last)
        } else {
            BandSignal::BandChanged(-1, -1)
        });

        let viewport_y = self.last_viewport_y;
        let margin = AUTO_SCROLL_MARGIN;
        if viewport_y < margin {
            // Python: max(-20, -(margin - viewportY) // 2)
            self.scroll_delta = (-(margin - viewport_y))
                .div_euclid(2)
                .max(-AUTO_SCROLL_MAX_DELTA);
            self.auto_scroll_active = true;
        } else if viewport_y > viewport_height - margin {
            // Python: min(20, (viewportY - viewportHeight + margin) // 2)
            self.scroll_delta = (viewport_y - viewport_height + margin)
                .div_euclid(2)
                .min(AUTO_SCROLL_MAX_DELTA);
            self.auto_scroll_active = true;
        } else {
            self.auto_scroll_active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> BandSelector {
        let mut selector = BandSelector::new(36, 4, 10);
        selector.set_item_count(20);
        selector.mouse_press(5, 10, 0);
        selector
    }

    /// stride 与初始态。
    #[test]
    fn stride_and_initial_state() {
        let selector = BandSelector::new(36, 4, 10);
        assert_eq!(selector.stride(), 40);
        assert_eq!(selector.item_count(), 0);
        assert!(selector.is_enabled());
        assert!(!selector.is_dragging());
        assert!(!selector.is_pending());
        assert!(!selector.band_visible());
        assert!(!selector.auto_scroll_active());
        assert_eq!(selector.scroll_value(), 0);
        assert_eq!(selector.band(), BandRect::default());
        assert!(selector.signals().is_empty());
        assert_eq!(AUTO_SCROLL_INTERVAL_MS, 30);
        assert_eq!(AUTO_SCROLL_MARGIN, 30);
        assert_eq!(AUTO_SCROLL_MAX_DELTA, 20);
    }

    /// 小于 startDragDistance 的移动只保持 pending，不产生信号。
    #[test]
    fn small_move_stays_pending() {
        let mut selector = started();
        assert!(selector.is_pending());
        // 曼哈顿距离 3 + 4 = 7 < 10
        assert!(!selector.mouse_move(8, 14, 400));
        assert!(selector.is_pending());
        assert!(!selector.is_dragging());
        assert!(!selector.band_visible());
        assert!(selector.signals().is_empty());
    }

    /// 越过阈值 → DragStarted + BandChanged，框矩形按起点/终点取包围盒。
    #[test]
    fn drag_starts_after_threshold() {
        let mut selector = started();
        assert!(selector.mouse_move(5, 20, 400));
        assert!(selector.is_dragging());
        assert!(!selector.is_pending());
        assert!(selector.band_visible());
        assert_eq!(
            selector.signals(),
            [BandSignal::DragStarted(0), BandSignal::BandChanged(0, 0),]
        );
        assert_eq!(
            selector.band(),
            BandRect {
                left: 5,
                top: 10,
                width: 1,
                height: 10,
            }
        );
    }

    /// 行列映射：`top // stride` / `bottom // stride`，越界截断到 itemCount-1。
    #[test]
    fn band_maps_rows_by_stride() {
        let mut selector = started();
        // 从 (5,10) 拖到 (5,95)：top=10 → 0，bottom=95 → 95//40 = 2
        selector.mouse_move(5, 95, 400);
        let signals = selector.take_signals();
        assert_eq!(
            signals,
            [BandSignal::DragStarted(0), BandSignal::BandChanged(0, 2),]
        );
        assert!(selector.signals().is_empty());
        assert_eq!(selector.band().height, 85);

        // 向上拖到负数：top = min(startY=10, -30) = -30，
        // bottom = max(10, -30) = 10 → height 40；first = max(0, -30//40=-1) = 0，
        // last = min(19, 10//40=0) = 0 → (0, 0)（bottom 不会跟着取负数）。
        selector.mouse_move(5, -30, 400);
        assert_eq!(selector.band().top, -30);
        assert_eq!(selector.band().height, 40);
        assert_eq!(selector.take_signals(), [BandSignal::BandChanged(0, 0)]);
    }

    /// itemCount 为 0 → `last = min(-1, …)` → 发 (-1, -1)。
    #[test]
    fn empty_list_reports_minus_one() {
        let mut selector = BandSelector::new(36, 4, 10);
        selector.mouse_press(0, 0, 0);
        selector.mouse_move(0, 50, 400);
        assert_eq!(
            selector.take_signals(),
            [BandSignal::DragStarted(0), BandSignal::BandChanged(-1, -1),]
        );
    }

    /// 上边界自动滚动：`-(margin - viewportY) // 2`，向下取整 + 夹 -20。
    #[test]
    fn auto_scroll_up_delta_uses_floor_division() {
        // viewportY = 10 → -(20)//2 = -10
        let mut selector = BandSelector::new(36, 4, 10);
        selector.set_item_count(20);
        selector.mouse_press(5, 0, 0);
        selector.mouse_move(5, 10, 400);
        assert!(selector.auto_scroll_active());
        assert_eq!(selector.scroll_delta(), -10);

        // viewportY = 29 → -(1)//2 = -1（Python 向下取整，不是 0）
        let mut near = BandSelector::new(36, 4, 10);
        near.set_item_count(20);
        near.mouse_press(5, 19, 0);
        near.mouse_move(5, 29, 400);
        assert!(near.auto_scroll_active());
        assert_eq!(near.scroll_delta(), -1);

        // viewportY = -30 → -(60)//2 = -30 → 夹到 -20
        let mut far = BandSelector::new(36, 4, 10);
        far.set_item_count(20);
        far.mouse_press(5, -40, 0);
        far.mouse_move(5, -30, 400);
        assert!(far.auto_scroll_active());
        assert_eq!(far.scroll_delta(), -20);
    }

    /// 下边界自动滚动：`(viewportY - viewportHeight + margin) // 2`，夹 +20。
    #[test]
    fn auto_scroll_down_delta_clamps() {
        let mut selector = BandSelector::new(36, 4, 10);
        selector.set_item_count(20);
        selector.mouse_press(5, 0, 0);
        // viewportY = 390 → 390 - 400 + 30 = 20 → //2 = 10
        selector.mouse_move(5, 390, 400);
        assert!(selector.auto_scroll_active());
        assert_eq!(selector.scroll_delta(), 10);

        // viewportY = 399 → 29//2 = 14
        selector.mouse_move(5, 399, 400);
        assert_eq!(selector.scroll_delta(), 14);

        // viewportY = 410 → 40//2 = 20
        selector.mouse_move(5, 410, 400);
        assert_eq!(selector.scroll_delta(), 20);

        // viewportY = 430 → 60//2 = 30 → 夹到 20
        selector.mouse_move(5, 430, 400);
        assert_eq!(selector.scroll_delta(), 20);
    }

    /// 中间区域停表。
    #[test]
    fn middle_region_stops_auto_scroll() {
        let mut selector = started();
        selector.mouse_move(5, 20, 400);
        assert!(selector.auto_scroll_active());
        selector.mouse_move(5, 200, 400);
        assert!(!selector.auto_scroll_active());
    }

    /// 自动滚动一档：滚动值夹在 [0, max]，并按新值重算框。
    #[test]
    fn auto_scroll_moves_scroll_value_and_band() {
        let mut selector = started();
        selector.mouse_move(5, 20, 400);
        // viewportY = 20 → -(30-20)//2 = -5
        assert_eq!(selector.scroll_delta(), -5);
        selector.take_signals();

        // 已在 0 → clamp 后仍是 0，框不动，但依旧发一次 bandChanged。
        selector.on_auto_scroll(400, 1000);
        assert_eq!(selector.scroll_value(), 0);
        assert_eq!(selector.take_signals(), [BandSignal::BandChanged(0, 0)]);

        // 下边界：视口 390 → delta 10
        let mut down = BandSelector::new(36, 4, 10);
        down.set_item_count(20);
        down.mouse_press(5, 0, 0);
        down.mouse_move(5, 390, 400);
        down.take_signals();
        down.on_auto_scroll(400, 1000);
        assert_eq!(down.scroll_value(), 10);
        // lastViewportY = 390 - 0 = 390 → y = 400 → bottom//40 = 10
        assert_eq!(down.band().top, 0);
        assert_eq!(down.take_signals(), [BandSignal::BandChanged(0, 10)]);

        // 超过 max_value 时夹住
        let mut capped = BandSelector::new(36, 4, 10);
        capped.set_item_count(20);
        capped.mouse_press(5, 0, 0);
        capped.mouse_move(5, 390, 400);
        capped.on_auto_scroll(400, 5);
        assert_eq!(capped.scroll_value(), 5);
    }

    /// 松开：拖动中取消并消费事件；未拖动只清 pending。
    #[test]
    fn release_cancels_drag() {
        let mut selector = started();
        selector.mouse_move(5, 95, 400);
        selector.take_signals();
        assert!(selector.mouse_release());
        assert!(!selector.is_dragging());
        assert!(!selector.is_pending());
        assert!(!selector.band_visible());
        assert!(!selector.auto_scroll_active());
        assert_eq!(selector.signals(), [BandSignal::DragFinished]);

        let mut idle = started();
        assert!(!idle.mouse_release());
        assert!(!idle.is_pending());
        assert!(idle.signals().is_empty());
    }

    /// setItemCount / setEnabled(False) / 失焦都会取消拖动。
    #[test]
    fn count_enabled_and_deactivate_cancel_drag() {
        let mut selector = started();
        selector.mouse_move(5, 95, 400);
        selector.take_signals();
        selector.set_item_count(3);
        assert_eq!(selector.item_count(), 3);
        assert!(!selector.is_dragging());
        assert_eq!(selector.signals(), [BandSignal::DragFinished]);

        let mut disabled = started();
        disabled.mouse_move(5, 95, 400);
        disabled.take_signals();
        disabled.set_enabled(false);
        assert!(!disabled.is_dragging());
        assert_eq!(disabled.signals(), [BandSignal::DragFinished]);
        // 禁用后所有事件都不处理，连 pending 都不清。
        disabled.mouse_press(1, 1, 0);
        assert!(!disabled.is_pending());
        assert!(!disabled.mouse_move(9, 99, 400));
        assert!(!disabled.mouse_release());

        let mut blurred = started();
        blurred.mouse_move(5, 95, 400);
        blurred.take_signals();
        assert!(!blurred.window_deactivate());
        assert!(!blurred.is_dragging());
        assert_eq!(blurred.signals(), [BandSignal::DragFinished]);
    }

    /// 拖动中滚动条变化 → 按 `lastViewportY + value` 重算框。
    #[test]
    fn scroll_change_while_dragging_recomputes_band() {
        let mut selector = started();
        selector.mouse_move(5, 200, 400);
        selector.take_signals();
        // lastViewportY = 200 → value 40 → y = 240 → bottom//40 = 6
        selector.on_scroll_changed(40, 400);
        assert_eq!(selector.scroll_value(), 40);
        assert_eq!(selector.take_signals(), [BandSignal::BandChanged(0, 6)]);

        // 未拖动时只记值，不发信号。
        let mut idle = BandSelector::new(36, 4, 10);
        idle.on_scroll_changed(80, 400);
        assert_eq!(idle.scroll_value(), 80);
        assert!(idle.signals().is_empty());
    }

    /// 按下时用当前滚动值折算视口坐标（`startY - value`）。
    #[test]
    fn press_offsets_viewport_by_scroll_value() {
        let mut selector = BandSelector::new(36, 4, 10);
        selector.set_item_count(20);
        selector.on_scroll_changed(120, 400);
        selector.mouse_press(0, 200, 0);
        // 拖动到 250：lastViewportY = 250 - 120 = 130 → 不在边距内
        selector.mouse_move(0, 250, 400);
        assert!(!selector.auto_scroll_active());
        // `_updateBand(lastX, pos.y())` 收的是**控件坐标**（y=250），不是视口
        // 坐标：top = min(200, 250) = 200 → 200//40 = 5，bottom = 250 → 6。
        assert_eq!(selector.take_signals()[1], BandSignal::BandChanged(5, 6));
    }
}
