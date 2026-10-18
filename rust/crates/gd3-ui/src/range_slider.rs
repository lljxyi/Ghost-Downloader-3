//! 镜像 `app/view/components/range_slider.py`（331 行）：双端滑块 + 数值气泡
//! + 帧预览弹窗。
//!
//! Qt 侧是一个自绘 `QWidget`（`paintEvent` 手绘槽、手柄、气泡），本模块把
//! 「画什么」和「点了会怎样」都算成纯数据：
//!
//! - [`RangeSliderState`]：范围/双端值/拖动端/内联编辑端 + 预览帧游标；
//! - [`RangeSliderState::paint_geometry`]：槽与手柄的矩形（颜色常量另给）；
//! - [`RangeSliderState::label_rects`]：两个数值气泡的矩形（需要字体度量入参）；
//! - [`RangeSliderState::mouse_press`] / [`RangeSliderState::mouse_move`] /
//!   [`RangeSliderState::mouse_release`]：鼠标状态机；
//! - [`RangeSliderState::start_inline_edit`] / [`RangeSliderState::finish_inline_edit`]：
//!   点气泡后浮出的 `LineEdit`（56×24）与它的解析收尾。
//!
//! ## 实证结论（逐条对着源码抄的）
//!
//! - `setRange(min, max)` 的 `_maximum = max(minimum, maximum)`：`max < min`
//!   时**上界被抬到下界**；`_startValue` 用新上下界夹，`_endValue` 用
//!   **夹完的** `_startValue` 当下界（所以可能把 end 一起拖到 min）。
//! - `setValues(start, end)` 同样：end 的下界是**夹完的** start。
//! - `_toPixelX` 在 `span <= 0` 时返回 `HANDLE_RADIUS`（不是 0），且始终
//!   `HANDLE_RADIUS + ratio * (width - HANDLE_SIZE)` —— 槽的两端各留半个手柄。
//! - `_toValue` 在 `width - HANDLE_SIZE <= 0` 时返回 `_minimum`；`ratio` 先
//!   夹到 `[0, 1]`；取整用 Python `round()` = **半值取偶**（`round(0.5) == 0`，
//!   `round(1.5) == 2`），故走 [`crate::editors::py_round`]。
//! - `_labelRects`：`bubbleY = LABEL_HEIGHT - bubbleH` 可以是**负数**（气泡
//!   比标签区高时向上溢出），不做夹取；只有 X 被夹到 `[0, width - w]`。
//!   两气泡若 `startBX + startW + 2 > endBX` 则以 `mid = (startX + endX) / 2`
//!   为轴对称分开（左泡右边界 `mid - 1`，右泡左边界 `mid + 1`），**之后再夹**，
//!   所以夹取仍可能让两泡重叠——此时命中判定 `endRect` 优先。
//! - `mousePressEvent` 只在左键生效；`y < LABEL_HEIGHT` 时只处理气泡（不拖），
//!   命中顺序是 **end 再 start**；否则按「离哪个手柄近」选端，**等距取 start**，
//!   并且按下即立刻 `_updateDrag`（不是等到 move）。
//! - `_updateDrag` 只在值**真的变了**时才发 `rangeChanged`；内联编辑收尾
//!   (`_onInlineEditFinished`) 却**无条件**发一次（哪怕值没变）。
//! - `_onInlineEditFinished` 的解析失败（`ValueError`/`TypeError`）只
//!   `update()` 后返回，**不发信号**；解析成功则先夹再赋值再发信号。
//! - `_startInlineEdit` 若已有 `LineEdit` 在编辑，**先**调用
//!   `_onInlineEditFinished()`（会把旧编辑的文本解析并可能发信号），再开新的。
//! - `_updatePreview`：`_previewFrameIndex` 初值 `-1`，值没变就**不重取帧**；
//!   取帧返回 `None` → 隐藏并**把游标重置为 -1**（下次同值会再取一次）。
//!   弹窗位置 `(globalX - w/2, globalY - h - PREVIEW_MARGIN)`，尺寸固定
//!   `(PREVIEW_W + 2*PREVIEW_BORDER, PREVIEW_H + 2*PREVIEW_BORDER)` = 164×94。
//! - `leaveEvent` / `hideEvent` 都调 `_hidePreview()`。

use crate::editors::py_round;
use crate::labels::Advance;

/// `HANDLE_SIZE = 22`（手柄直径）。
pub const HANDLE_SIZE: i32 = 22;
/// `HANDLE_RADIUS = HANDLE_SIZE // 2`。
pub const HANDLE_RADIUS: i32 = 11;
/// `INNER_RADIUS = 5`（手柄内圆半径）。
pub const INNER_RADIUS: i32 = 5;
/// `GROOVE_HEIGHT = 4`。
pub const GROOVE_HEIGHT: i32 = 4;
/// `GROOVE_RADIUS = 2`。
pub const GROOVE_RADIUS: i32 = 2;
/// `LABEL_HEIGHT = 24`（顶部标签区高度）。
pub const LABEL_HEIGHT: i32 = 24;
/// `LABEL_GAP = 4`。
pub const LABEL_GAP: i32 = 4;
/// `LABEL_PADDING_H = 8`（气泡左右内边距）。
pub const LABEL_PADDING_H: i32 = 8;
/// `LABEL_PADDING_V = 6`（气泡上下内边距）。
pub const LABEL_PADDING_V: i32 = 6;
/// `LABEL_BORDER_RADIUS = 4`。
pub const LABEL_BORDER_RADIUS: i32 = 4;
/// `LABEL_FONT_SIZE = 12`。
pub const LABEL_FONT_SIZE: i32 = 12;
/// `PREVIEW_W = 160`。
pub const PREVIEW_W: i32 = 160;
/// `PREVIEW_H = 90`。
pub const PREVIEW_H: i32 = 90;
/// `PREVIEW_BORDER = 2`。
pub const PREVIEW_BORDER: i32 = 2;
/// `PREVIEW_BORDER_RADIUS = 4`。
pub const PREVIEW_BORDER_RADIUS: i32 = 4;
/// `PREVIEW_MARGIN = 6`（弹窗底边与鼠标的间距）。
pub const PREVIEW_MARGIN: i32 = 6;
/// `self._lineEdit.setFixedSize(56, LABEL_HEIGHT)`。
pub const INLINE_EDIT_W: i32 = 56;
/// `self._lineEdit.move(int(max(0, min(self.width() - 56, hx - 28))), 0)` 里的 28。
pub const INLINE_EDIT_HALF: i32 = 28;

/// 一个带 alpha 的颜色（Qt `QColor(r, g, b, a)`）。
pub type Rgba = (u8, u8, u8, u8);

/// 浮点矩形（Qt `QRectF`）。只用得到 `contains`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectF {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// `QRectF.contains(QPointF)`：左/上闭，右/下开。
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// 拖的是哪一端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Start,
    End,
}

/// `rangeChanged(start, end)` 的载荷。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: i32,
    pub end: i32,
}

/// 一次按下之后要做什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PressOutcome {
    /// 非左键，或落点不在任何手柄/气泡上（Qt 里 `mousePressEvent` 只对左键
    /// 有反应，但落在控件任何位置都会选中最近的手柄——所以这里只有非左键
    /// 才会是 `Ignored`）。
    Ignored,
    /// 点中了数值气泡 → 开内联编辑。`finished` 是**旧编辑**收尾时可能发出的
    /// `rangeChanged`（`_startInlineEdit` 先收尾旧的）。
    Editing {
        finished: Option<Range>,
        edit: InlineEdit,
    },
    /// 选中了某一端并立刻拖了一次。`changed` 是这次拖动发出的信号。
    Dragging {
        handle: Handle,
        changed: Option<Range>,
    },
}

/// 内联编辑框的几何与初值（`LineEdit(self)` + `setFixedSize(56, LABEL_HEIGHT)`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineEdit {
    pub handle: Handle,
    /// `self._formatter(value)`。
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// `mouseMoveEvent` 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveOutcome {
    /// 拖动中且值变了才有。
    pub changed: Option<Range>,
    /// 预览帧要做什么。
    pub preview: PreviewAction,
}

/// `_updatePreview` 的结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewAction {
    /// 没有 provider、或帧游标没变（Qt 里这时只是重新定位弹窗）。
    Idle,
    /// 游标变了 → 调 `_previewProvider(value)` 取帧，取到后调
    /// [`RangeSliderState::on_preview_frame`]`(true)`，取不到传 `false`。
    Fetch(i32),
    /// 取不到帧，已隐藏（`_hidePreview`）。
    Hide,
}

/// 自绘几何（`paintEvent` 里除文字外的部分）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintGeometry {
    /// 未激活槽：`QRectF(HANDLE_RADIUS, grooveY, width - HANDLE_SIZE, GROOVE_HEIGHT)`。
    pub groove: RectF,
    /// 激活槽：仅 `endX > startX` 时存在。
    pub active: Option<RectF>,
    /// 两个手柄的圆心 X（start, end）。
    pub handle_x: [f64; 2],
    /// 手柄圆心 Y = `LABEL_HEIGHT + LABEL_GAP + HANDLE_RADIUS`。
    pub handle_center_y: f64,
}

/// 双端滑块的状态面。
#[derive(Clone, Debug, PartialEq)]
pub struct RangeSliderState {
    minimum: i32,
    maximum: i32,
    start_value: i32,
    end_value: i32,
    dragging: Option<Handle>,
    editing: Option<Handle>,
    /// `self._lineEdit is not None`。
    edit_text: Option<String>,
    /// `_previewProvider` 是否装上了（Rust 侧只记「有没有」，回调归调用方）。
    preview_enabled: bool,
    /// `_previewFrameIndex`，`-1` = 无。
    preview_frame_index: i32,
    /// `_preview is not None`。
    preview_created: bool,
    /// `_preview.isVisible()`。
    preview_visible: bool,
}

impl Default for RangeSliderState {
    fn default() -> Self {
        Self::new()
    }
}

impl RangeSliderState {
    /// `__init__`：`(0, 100, 0, 100)`，无拖动、无编辑、无预览。
    pub fn new() -> Self {
        Self {
            minimum: 0,
            maximum: 100,
            start_value: 0,
            end_value: 100,
            dragging: None,
            editing: None,
            edit_text: None,
            preview_enabled: false,
            preview_frame_index: -1,
            preview_created: false,
            preview_visible: false,
        }
    }

    /// `setFixedHeight(LABEL_HEIGHT + LABEL_GAP + HANDLE_SIZE)` = 50。
    pub fn fixed_height() -> i32 {
        LABEL_HEIGHT + LABEL_GAP + HANDLE_SIZE
    }

    pub fn minimum(&self) -> i32 {
        self.minimum
    }

    pub fn maximum(&self) -> i32 {
        self.maximum
    }

    pub fn start_value(&self) -> i32 {
        self.start_value
    }

    pub fn end_value(&self) -> i32 {
        self.end_value
    }

    pub fn dragging(&self) -> Option<Handle> {
        self.dragging
    }

    /// `self._lineEdit is not None`（编辑中时能读到当前文本）。
    pub fn editing_text(&self) -> Option<&str> {
        self.edit_text.as_deref()
    }

    /// `setPreviewProvider(provider)`：`None` → 关掉。
    pub fn set_preview_enabled(&mut self, enabled: bool) {
        self.preview_enabled = enabled;
    }

    pub fn preview_frame_index(&self) -> i32 {
        self.preview_frame_index
    }

    pub fn preview_visible(&self) -> bool {
        self.preview_visible
    }

    /// `setRange(minimum, maximum)`。
    pub fn set_range(&mut self, minimum: i32, maximum: i32) {
        self.minimum = minimum;
        self.maximum = minimum.max(maximum);
        self.start_value = self.minimum.max(self.start_value.min(self.maximum));
        self.end_value = self.start_value.max(self.end_value.min(self.maximum));
    }

    /// `setValues(start, end)`。
    pub fn set_values(&mut self, start: i32, end: i32) {
        self.start_value = self.minimum.max(start.min(self.maximum));
        self.end_value = self.start_value.max(end.min(self.maximum));
    }

    /// `clear()`：回到满量程。
    pub fn clear(&mut self) {
        self.start_value = self.minimum;
        self.end_value = self.maximum;
    }

    /// `_toPixelX(value)`。
    pub fn to_pixel_x(&self, value: i32, width: i32) -> f64 {
        let span = self.maximum - self.minimum;
        if span <= 0 {
            return f64::from(HANDLE_RADIUS);
        }
        f64::from(HANDLE_RADIUS)
            + f64::from(value - self.minimum) / f64::from(span) * f64::from(width - HANDLE_SIZE)
    }

    /// `_toValue(x)`。取整走 [`py_round`]（半值取偶）。
    pub fn to_value(&self, x: f64, width: i32) -> i32 {
        let groove_len = width - HANDLE_SIZE;
        if groove_len <= 0 {
            return self.minimum;
        }
        let ratio = ((x - f64::from(HANDLE_RADIUS)) / f64::from(groove_len)).clamp(0.0, 1.0);
        self.minimum + py_round(ratio * f64::from(self.maximum - self.minimum))
    }

    /// `paintEvent` 的几何部分。
    pub fn paint_geometry(&self, width: i32) -> PaintGeometry {
        let handle_center_y = f64::from(LABEL_HEIGHT + LABEL_GAP + HANDLE_RADIUS);
        let groove_y = handle_center_y - f64::from(GROOVE_HEIGHT) / 2.0;
        let start_x = self.to_pixel_x(self.start_value, width);
        let end_x = self.to_pixel_x(self.end_value, width);
        PaintGeometry {
            groove: RectF::new(
                f64::from(HANDLE_RADIUS),
                groove_y,
                f64::from(width - HANDLE_SIZE),
                f64::from(GROOVE_HEIGHT),
            ),
            active: (end_x > start_x)
                .then(|| RectF::new(start_x, groove_y, end_x - start_x, f64::from(GROOVE_HEIGHT))),
            handle_x: [start_x, end_x],
            handle_center_y,
        }
    }

    /// 未激活槽的填充色：`QColor(255,255,255,115)` / `QColor(0,0,0,100)`。
    pub fn groove_color(dark: bool) -> Rgba {
        if dark {
            (255, 255, 255, 115)
        } else {
            (0, 0, 0, 100)
        }
    }

    /// 手柄的（填充, 描边）：深色 `(69,69,69)` + `(0,0,0,90)`，
    /// 浅色 `(255,255,255)` + `(0,0,0,25)`。
    pub fn handle_colors(dark: bool) -> (Rgba, Rgba) {
        if dark {
            ((69, 69, 69, 255), (0, 0, 0, 90))
        } else {
            ((255, 255, 255, 255), (0, 0, 0, 25))
        }
    }

    /// 数值气泡的（填充, 描边, 文字）：深色 `(50,50,50)` + `(255,255,255,15)`
    /// + 白字，浅色 `(249,249,249)` + `(0,0,0,15)` + 黑字。
    pub fn label_colors(dark: bool) -> (Rgba, Rgba, (u8, u8, u8)) {
        if dark {
            ((50, 50, 50, 255), (255, 255, 255, 15), (255, 255, 255))
        } else {
            ((249, 249, 249, 255), (0, 0, 0, 15), (0, 0, 0))
        }
    }

    /// 预览弹窗的（填充, 描边）：深色 `(50,50,50)` + `(255,255,255,30)`，
    /// 浅色 `(249,249,249)` + `(0,0,0,25)`。
    pub fn preview_colors(dark: bool) -> (Rgba, Rgba) {
        if dark {
            ((50, 50, 50, 255), (255, 255, 255, 30))
        } else {
            ((249, 249, 249, 255), (0, 0, 0, 25))
        }
    }

    /// `PreviewPopup.setFixedSize(...)` 的尺寸：164×94。
    pub fn preview_size() -> (i32, i32) {
        (
            PREVIEW_W + PREVIEW_BORDER * 2,
            PREVIEW_H + PREVIEW_BORDER * 2,
        )
    }

    /// `_labelRects()`：两个数值气泡。`metrics.advance` 是
    /// `QFontMetrics.horizontalAdvance`，`metrics.font_height` 是 `fm.height()`。
    pub fn label_rects(
        &self,
        width: i32,
        metrics: &LabelMetrics<'_>,
        formatter: &dyn Fn(i32) -> String,
    ) -> (RectF, RectF) {
        let start_x = self.to_pixel_x(self.start_value, width);
        let end_x = self.to_pixel_x(self.end_value, width);
        let bubble_h = f64::from(metrics.font_height + LABEL_PADDING_V * 2);
        let bubble_y = f64::from(LABEL_HEIGHT) - bubble_h;

        let start_w =
            f64::from((metrics.advance)(&formatter(self.start_value)) + LABEL_PADDING_H * 2);
        let end_w = f64::from((metrics.advance)(&formatter(self.end_value)) + LABEL_PADDING_H * 2);
        let mut start_bx = start_x - start_w / 2.0;
        let mut end_bx = end_x - end_w / 2.0;

        if start_bx + start_w + 2.0 > end_bx {
            let mid = (start_x + end_x) / 2.0;
            start_bx = mid - start_w - 1.0;
            end_bx = mid + 1.0;
        }

        let start_bx = start_bx.clamp(0.0, f64::from(width) - start_w);
        let end_bx = end_bx.clamp(0.0, f64::from(width) - end_w);

        (
            RectF::new(start_bx, bubble_y, start_w, bubble_h),
            RectF::new(end_bx, bubble_y, end_w, bubble_h),
        )
    }

    /// `mousePressEvent`。`left_button` = `event.button() == LeftButton`。
    pub fn mouse_press(
        &mut self,
        x: f64,
        y: f64,
        width: i32,
        left_button: bool,
        metrics: &LabelMetrics<'_>,
        formatter: &dyn Fn(i32) -> String,
    ) -> PressOutcome {
        if !left_button {
            return PressOutcome::Ignored;
        }

        if y < f64::from(LABEL_HEIGHT) {
            let (start_rect, end_rect) = self.label_rects(width, metrics, formatter);
            let handle = if end_rect.contains(x, y) {
                Some(Handle::End)
            } else if start_rect.contains(x, y) {
                Some(Handle::Start)
            } else {
                None
            };
            return match handle {
                Some(handle) => {
                    let (finished, edit) = self.start_inline_edit(handle, width, formatter);
                    PressOutcome::Editing { finished, edit }
                }
                None => PressOutcome::Ignored,
            };
        }

        let start_x = self.to_pixel_x(self.start_value, width);
        let end_x = self.to_pixel_x(self.end_value, width);
        let handle = if (x - start_x).abs() <= (x - end_x).abs() {
            Handle::Start
        } else {
            Handle::End
        };
        self.dragging = Some(handle);
        PressOutcome::Dragging {
            handle,
            changed: self.update_drag(x, width),
        }
    }

    /// `mouseMoveEvent`：拖动跟手 + 预览跟随，两件事**都会**发生。
    pub fn mouse_move(&mut self, x: f64, width: i32) -> MoveOutcome {
        let changed = if self.dragging.is_some() {
            self.update_drag(x, width)
        } else {
            None
        };
        MoveOutcome {
            changed,
            preview: self.update_preview(x, width),
        }
    }

    /// `mouseReleaseEvent`：只在左键时松开拖动端。
    pub fn mouse_release(&mut self, left_button: bool) {
        if left_button {
            self.dragging = None;
        }
    }

    /// `leaveEvent` / `hideEvent`。
    pub fn hide_preview(&mut self) {
        self.preview_visible = false;
        self.preview_frame_index = -1;
    }

    /// 取帧结果回填：`Some` → 弹窗显示，`None` → `_hidePreview()`。
    pub fn on_preview_frame(&mut self, available: bool) {
        if available {
            self.preview_created = true;
            self.preview_visible = true;
        } else {
            self.hide_preview();
        }
    }

    /// 弹窗应放的位置：`(globalX - w/2, globalY - h - PREVIEW_MARGIN)`。
    /// 只有弹窗已创建且可见时才有值（Qt 里 `isVisible()` 为假就跳过定位）。
    pub fn preview_position(&self, global_x: i32, global_y: i32) -> Option<(i32, i32)> {
        if !(self.preview_created && self.preview_visible) {
            return None;
        }
        let (w, h) = Self::preview_size();
        Some((global_x - w / 2, global_y - h - PREVIEW_MARGIN))
    }

    /// `_updateDrag(x)`：只在值变了时返回信号载荷。
    pub fn update_drag(&mut self, x: f64, width: i32) -> Option<Range> {
        let value = self.to_value(x, width);
        match self.dragging? {
            Handle::Start => {
                let value = self.minimum.max(value.min(self.end_value));
                if value != self.start_value {
                    self.start_value = value;
                    Some(Range {
                        start: self.start_value,
                        end: self.end_value,
                    })
                } else {
                    None
                }
            }
            Handle::End => {
                let value = self.start_value.max(value.min(self.maximum));
                if value != self.end_value {
                    self.end_value = value;
                    Some(Range {
                        start: self.start_value,
                        end: self.end_value,
                    })
                } else {
                    None
                }
            }
        }
    }

    /// `_updatePreview(x)`。
    fn update_preview(&mut self, x: f64, width: i32) -> PreviewAction {
        if !self.preview_enabled {
            return PreviewAction::Idle;
        }
        let value = self.to_value(x, width);
        if value != self.preview_frame_index {
            self.preview_frame_index = value;
            return PreviewAction::Fetch(value);
        }
        PreviewAction::Idle
    }

    /// `_startInlineEdit(handle)`。
    ///
    /// 已有编辑框时**先**收尾（`_onInlineEditFinished`），其信号载荷由返回值
    /// 的第一项带出。返回 `(旧编辑的信号, 新编辑框)`。
    pub fn start_inline_edit(
        &mut self,
        handle: Handle,
        width: i32,
        formatter: &dyn Fn(i32) -> String,
    ) -> (Option<Range>, InlineEdit) {
        let finished = if self.edit_text.is_some() {
            let text = self.edit_text.clone().unwrap_or_default();
            self.finish_inline_edit(&text)
        } else {
            None
        };

        let value = match handle {
            Handle::Start => self.start_value,
            Handle::End => self.end_value,
        };
        let hx = self.to_pixel_x(value, width);
        let text = formatter(value);
        self.editing = Some(handle);
        self.edit_text = Some(text.clone());

        let x = (f64::from(width - INLINE_EDIT_W)).min(hx - f64::from(INLINE_EDIT_HALF));
        let x = 0.0_f64.max(x) as i32;
        (
            finished,
            InlineEdit {
                handle,
                text,
                x,
                y: 0,
                w: INLINE_EDIT_W,
                h: LABEL_HEIGHT,
            },
        )
    }

    /// `_onInlineEditFinished()`：解析失败返回 `None`（不发信号），成功则
    /// **无条件**发一次 `rangeChanged`（哪怕值没变）。
    pub fn finish_inline_edit(&mut self, text: &str) -> Option<Range> {
        let handle = self.editing.take()?;
        self.edit_text = None;

        let value = py_int(text)?;
        match handle {
            Handle::Start => {
                self.start_value = self.minimum.max(value.min(self.end_value));
            }
            Handle::End => {
                self.end_value = self.start_value.max(value.min(self.maximum));
            }
        }
        Some(Range {
            start: self.start_value,
            end: self.end_value,
        })
    }
}

/// `_labelRects` 需要的字体度量（`QFontMetrics` 的两个方法）。
pub struct LabelMetrics<'a> {
    /// `fm.horizontalAdvance(text)`。
    pub advance: Advance<'a>,
    /// `fm.height()`。
    pub font_height: i32,
}

/// 默认 `formatter=str`。
pub fn default_formatter(value: i32) -> String {
    value.to_string()
}

/// 默认 `parser=int`：`int()` 接受首尾空白、正负号、数字间的单个下划线。
///
/// 偏差：Python `int()` 还接受非 ASCII 数字（`int("１２")` = 12）与任意
/// Unicode 空白；本侧只认 ASCII 数字与 `str::trim` 的空白集合。滑块上
/// 手输全角数字的场景可以忽略，且 `formatter` 的产物恒为 ASCII。
pub fn py_int(text: &str) -> Option<i32> {
    let trimmed = text.trim();
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() {
        return None;
    }

    let mut value: i32 = 0;
    let mut prev_underscore = false;
    for (index, ch) in digits.char_indices() {
        if ch == '_' {
            // 下划线只能夹在两个数字之间：首字符、末字符、连续下划线都非法。
            if index == 0 || prev_underscore || index + 1 == digits.len() {
                return None;
            }
            prev_underscore = true;
            continue;
        }
        let digit = ch.to_digit(10)?;
        value = value.checked_mul(10)?.checked_add(digit as i32)?;
        prev_underscore = false;
    }
    Some(if negative { -value } else { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(advance: &dyn Fn(&str) -> i32, font_height: i32) -> LabelMetrics<'_> {
        LabelMetrics {
            advance,
            font_height,
        }
    }

    /// 每个字符 8px、行高 16px 的假字体。
    fn fake_advance(text: &str) -> i32 {
        text.chars().count() as i32 * 8
    }

    #[test]
    fn constants_mirror_python_literals() {
        assert_eq!(HANDLE_SIZE, 22);
        assert_eq!(HANDLE_RADIUS, 11);
        assert_eq!(INNER_RADIUS, 5);
        assert_eq!(GROOVE_HEIGHT, 4);
        assert_eq!(GROOVE_RADIUS, 2);
        assert_eq!(LABEL_HEIGHT, 24);
        assert_eq!(LABEL_GAP, 4);
        assert_eq!(LABEL_PADDING_H, 8);
        assert_eq!(LABEL_PADDING_V, 6);
        assert_eq!(LABEL_BORDER_RADIUS, 4);
        assert_eq!(LABEL_FONT_SIZE, 12);
        assert_eq!(PREVIEW_W, 160);
        assert_eq!(PREVIEW_H, 90);
        assert_eq!(PREVIEW_BORDER, 2);
        assert_eq!(PREVIEW_BORDER_RADIUS, 4);
        assert_eq!(PREVIEW_MARGIN, 6);
        assert_eq!(RangeSliderState::fixed_height(), 50);
        assert_eq!(RangeSliderState::preview_size(), (164, 94));
    }

    #[test]
    fn initial_state_is_full_range() {
        let slider = RangeSliderState::new();
        assert_eq!(slider.minimum(), 0);
        assert_eq!(slider.maximum(), 100);
        assert_eq!(slider.start_value(), 0);
        assert_eq!(slider.end_value(), 100);
        assert_eq!(slider.dragging(), None);
        assert_eq!(slider.editing_text(), None);
        assert_eq!(slider.preview_frame_index(), -1);
        assert!(!slider.preview_visible());
    }

    /// `_maximum = max(minimum, maximum)`，且 end 的下界是夹完的 start。
    #[test]
    fn set_range_pulls_bounds_and_reclamps() {
        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        slider.set_range(10, 5);
        assert_eq!(slider.minimum(), 10);
        assert_eq!(slider.maximum(), 10, "max < min 时上界被抬到下界");
        assert_eq!(slider.start_value(), 10);
        assert_eq!(slider.end_value(), 10);

        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        slider.set_range(30, 50);
        assert_eq!((slider.start_value(), slider.end_value()), (30, 50));
    }

    #[test]
    fn set_values_clamps_end_against_clamped_start() {
        let mut slider = RangeSliderState::new();
        slider.set_values(50, 20);
        assert_eq!((slider.start_value(), slider.end_value()), (50, 50));
        slider.set_values(-5, 500);
        assert_eq!((slider.start_value(), slider.end_value()), (0, 100));
    }

    #[test]
    fn clear_restores_full_range() {
        let mut slider = RangeSliderState::new();
        slider.set_range(10, 90);
        slider.set_values(30, 40);
        slider.clear();
        assert_eq!((slider.start_value(), slider.end_value()), (10, 90));
    }

    /// `span <= 0` 时 `_toPixelX` 返回 `HANDLE_RADIUS`。
    #[test]
    fn to_pixel_x_reserves_half_handle_on_both_sides() {
        let mut slider = RangeSliderState::new();
        assert_eq!(slider.to_pixel_x(0, 300), 11.0);
        assert!((slider.to_pixel_x(50, 300) - 150.0).abs() < 1e-9);
        assert!((slider.to_pixel_x(100, 300) - 289.0).abs() < 1e-9);
        slider.set_range(5, 5);
        assert_eq!(slider.to_pixel_x(5, 300), 11.0);
    }

    /// `width - HANDLE_SIZE <= 0` 时 `_toValue` 返回 `_minimum`。
    #[test]
    fn to_value_short_groove_falls_back_to_minimum() {
        let mut slider = RangeSliderState::new();
        slider.set_range(7, 70);
        assert_eq!(slider.to_value(150.0, 22), 7);
        assert_eq!(slider.to_value(150.0, 10), 7);
    }

    #[test]
    fn to_value_clamps_ratio() {
        let mut slider = RangeSliderState::new();
        slider.set_range(10, 20);
        assert_eq!(slider.to_value(0.0, 300), 10);
        assert_eq!(slider.to_value(400.0, 300), 20);
        // 中点 150 → ratio 139/278 = 0.5 → round(5.0) = 5
        assert_eq!(slider.to_value(150.0, 300), 15);
    }

    /// Python `round()` 是半值取偶：`round(0.5) == 0`、`round(1.5) == 2`。
    #[test]
    fn to_value_uses_bankers_rounding() {
        let mut slider = RangeSliderState::new();
        slider.set_range(0, 1);
        assert_eq!(slider.to_value(150.0, 300), 0, "round(0.5) 应为 0");
        slider.set_range(0, 3);
        assert_eq!(slider.to_value(150.0, 300), 2, "round(1.5) 应为 2");
        slider.set_range(0, 5);
        assert_eq!(slider.to_value(150.0, 300), 2, "round(2.5) 应为 2");
    }

    #[test]
    fn paint_geometry_places_groove_and_handles() {
        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        let geo = slider.paint_geometry(300);
        assert_eq!(geo.groove.y, 37.0);
        assert_eq!(geo.groove.x, 11.0);
        assert_eq!(geo.groove.w, 278.0);
        assert_eq!(geo.groove.h, 4.0);
        assert_eq!(geo.handle_center_y, 39.0);
        let active = geo.active.expect("end > start 时激活槽存在");
        assert!((active.x - 66.6).abs() < 1e-9);
        assert!((active.w - (233.4 - 66.6)).abs() < 1e-9);

        // end == start → 没有激活槽。
        slider.set_values(50, 50);
        assert!(slider.paint_geometry(300).active.is_none());
    }

    #[test]
    fn label_rects_centers_bubbles_on_handles() {
        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        let m = metrics(&fake_advance, 16);
        let (start, end) = slider.label_rects(300, &m, &default_formatter);
        // bubbleH = 16 + 12 = 28, bubbleY = 24 - 28 = -4（向上溢出，不夹）
        assert_eq!(start.h, 28.0);
        assert_eq!(start.y, -4.0);
        assert_eq!(start.w, 32.0);
        assert!((start.x - 50.6).abs() < 1e-9);
        assert!((end.x - 217.4).abs() < 1e-9);
    }

    /// 两泡相撞时以中轴分开，左右各留 1px 间隙。
    #[test]
    fn label_rects_split_overlapping_bubbles() {
        let mut slider = RangeSliderState::new();
        slider.set_values(48, 52);
        let m = metrics(&fake_advance, 16);
        let (start, end) = slider.label_rects(300, &m, &default_formatter);
        assert_eq!(start.x, 117.0);
        assert_eq!(end.x, 151.0);
        assert_eq!(start.x + start.w + 2.0, end.x);
    }

    /// 分开之后仍会夹到控件内（两端 0 / width-w）。
    #[test]
    fn label_rects_clamp_to_widget_bounds() {
        let slider = RangeSliderState::new();
        let m = metrics(&fake_advance, 16);
        let (start, end) = slider.label_rects(300, &m, &default_formatter);
        // startW = 8 + 16 = 24 → startBX = 11 - 12 = -1 → 夹到 0；
        // endW = 24 + 16 = 40 → endBX = 289 - 20 = 269 → 夹到 300 - 40 = 260。
        assert_eq!(start.w, 24.0);
        assert_eq!(end.w, 40.0);
        assert_eq!(start.x, 0.0);
        assert_eq!(end.x, 260.0);
    }

    /// start == end == minimum 时两泡夹取后仍重叠，命中判定 end 优先。
    #[test]
    fn press_on_overlapping_bubbles_prefers_end() {
        let mut slider = RangeSliderState::new();
        slider.set_values(0, 0);
        let m = metrics(&fake_advance, 16);
        let outcome = slider.mouse_press(15.0, 10.0, 300, true, &m, &default_formatter);
        match outcome {
            PressOutcome::Editing { edit, finished } => {
                assert_eq!(edit.handle, Handle::End);
                assert_eq!(finished, None, "没有旧编辑就没有信号");
                assert_eq!(edit.text, "0");
            }
            other => panic!("应命中气泡，实际 {other:?}"),
        }
        assert_eq!(slider.editing_text(), Some("0"));
    }

    #[test]
    fn press_outside_bubbles_ignored() {
        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        let m = metrics(&fake_advance, 16);
        assert_eq!(
            slider.mouse_press(150.0, 10.0, 300, true, &m, &default_formatter),
            PressOutcome::Ignored
        );
        // 非左键：连拖动端都不选。
        assert_eq!(
            slider.mouse_press(150.0, 40.0, 300, false, &m, &default_formatter),
            PressOutcome::Ignored
        );
        assert_eq!(slider.dragging(), None);
    }

    /// 等距时取 start，并且按下即拖（`_updateDrag` 立刻执行）。
    #[test]
    fn press_picks_nearest_handle_and_drags_immediately() {
        let mut slider = RangeSliderState::new();
        let m = metrics(&fake_advance, 16);
        let outcome = slider.mouse_press(150.0, 40.0, 300, true, &m, &default_formatter);
        assert_eq!(
            outcome,
            PressOutcome::Dragging {
                handle: Handle::Start,
                changed: Some(Range {
                    start: 50,
                    end: 100
                }),
            }
        );
        assert_eq!(slider.dragging(), Some(Handle::Start));
    }

    #[test]
    fn update_drag_clamps_and_only_reports_changes() {
        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        slider.mouse_release(true);
        // 没有拖动端时 update_drag 什么都不做。
        assert_eq!(slider.update_drag(150.0, 300), None);

        slider.dragging = Some(Handle::Start);
        // x=289 → 100，被 end 夹到 80。
        assert_eq!(
            slider.update_drag(289.0, 300),
            Some(Range { start: 80, end: 80 })
        );
        assert_eq!(slider.update_drag(289.0, 300), None, "值没变就不发信号");

        // end 也已是 80：被 start 夹回 80，值没变 → 不发信号。
        slider.dragging = Some(Handle::End);
        assert_eq!(slider.update_drag(0.0, 300), None);

        // 换个场景验 End 分支真的会发信号：end 被拉到 start 上。
        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        slider.dragging = Some(Handle::End);
        assert_eq!(
            slider.update_drag(0.0, 300),
            Some(Range { start: 20, end: 20 })
        );
    }

    #[test]
    fn mouse_release_only_clears_on_left_button() {
        let mut slider = RangeSliderState::new();
        slider.dragging = Some(Handle::End);
        slider.mouse_release(false);
        assert_eq!(slider.dragging(), Some(Handle::End));
        slider.mouse_release(true);
        assert_eq!(slider.dragging(), None);
    }

    #[test]
    fn inline_edit_geometry_and_finish() {
        let mut slider = RangeSliderState::new();
        slider.set_values(20, 80);
        let (finished, edit) = slider.start_inline_edit(Handle::Start, 300, &default_formatter);
        assert_eq!(finished, None);
        assert_eq!(edit.handle, Handle::Start);
        assert_eq!(edit.text, "20");
        // hx = 66.6 → 66.6 - 28 = 38.6 → int() 截断为 38
        assert_eq!(edit.x, 38);
        assert_eq!(edit.y, 0);
        assert_eq!(edit.w, 56);
        assert_eq!(edit.h, 24);

        // 解析成功 → 无条件发信号（哪怕值没变）。
        assert_eq!(
            slider.finish_inline_edit("20"),
            Some(Range { start: 20, end: 80 })
        );
        assert_eq!(slider.editing_text(), None);
        // 编辑框已关，再收尾什么都不做。
        assert_eq!(slider.finish_inline_edit("50"), None);
    }

    /// 解析失败（`ValueError`）只刷新，不发信号。
    #[test]
    fn inline_edit_parse_failure_emits_nothing() {
        let mut slider = RangeSliderState::new();
        slider.start_inline_edit(Handle::End, 300, &default_formatter);
        assert_eq!(slider.finish_inline_edit("abc"), None);
        assert_eq!((slider.start_value(), slider.end_value()), (0, 100));
        assert_eq!(slider.editing_text(), None);
    }

    /// 已有编辑框时先收尾旧的（会把旧文本解析并发信号），再开新的。
    #[test]
    fn starting_a_second_edit_finishes_the_first() {
        let mut slider = RangeSliderState::new();
        slider.start_inline_edit(Handle::Start, 300, &default_formatter);
        slider.edit_text = Some("40".to_string());
        let (finished, edit) = slider.start_inline_edit(Handle::End, 300, &default_formatter);
        assert_eq!(
            finished,
            Some(Range {
                start: 40,
                end: 100
            })
        );
        assert_eq!(edit.handle, Handle::End);
        assert_eq!(edit.text, "100");
        assert_eq!(slider.start_value(), 40);
    }

    #[test]
    fn preview_follows_frame_index() {
        let mut slider = RangeSliderState::new();
        // 没装 provider → 什么都不做。
        assert_eq!(slider.mouse_move(150.0, 300).preview, PreviewAction::Idle);

        slider.set_preview_enabled(true);
        assert_eq!(
            slider.mouse_move(150.0, 300).preview,
            PreviewAction::Fetch(50)
        );
        // 同值不重取帧。
        assert_eq!(slider.mouse_move(150.0, 300).preview, PreviewAction::Idle);
        assert_eq!(slider.preview_position(0, 0), None, "还没取到帧");

        slider.on_preview_frame(true);
        assert!(slider.preview_visible());
        assert_eq!(
            slider.preview_position(1000, 500),
            Some((1000 - 82, 500 - 94 - 6))
        );

        // 取不到帧 → 隐藏 + 游标重置为 -1（同值会再取一次）。
        slider.on_preview_frame(false);
        assert!(!slider.preview_visible());
        assert_eq!(slider.preview_frame_index(), -1);
        assert_eq!(
            slider.mouse_move(150.0, 300).preview,
            PreviewAction::Fetch(50)
        );
    }

    #[test]
    fn hide_preview_resets_cursor() {
        let mut slider = RangeSliderState::new();
        slider.set_preview_enabled(true);
        slider.mouse_move(150.0, 300);
        slider.on_preview_frame(true);
        slider.hide_preview();
        assert!(!slider.preview_visible());
        assert_eq!(slider.preview_frame_index(), -1);
        assert_eq!(slider.preview_position(0, 0), None);
    }

    #[test]
    fn py_int_mirrors_python_int_literal_rules() {
        assert_eq!(py_int("42"), Some(42));
        assert_eq!(py_int("  -7 "), Some(-7));
        assert_eq!(py_int("+7"), Some(7));
        assert_eq!(py_int("1_000"), Some(1000));
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("   "), None);
        assert_eq!(py_int("abc"), None);
        assert_eq!(py_int("1.5"), None);
        assert_eq!(py_int("0x10"), None);
        // 下划线不能当边界，也不能连续。
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("1_"), None);
        assert_eq!(py_int("1__0"), None);
        // 溢出 i32 也不收（Python 是大整数，滑块用不到）。
        assert_eq!(py_int("99999999999"), None);
    }

    #[test]
    fn colors_mirror_python_literals() {
        assert_eq!(RangeSliderState::groove_color(true), (255, 255, 255, 115));
        assert_eq!(RangeSliderState::groove_color(false), (0, 0, 0, 100));
        assert_eq!(
            RangeSliderState::handle_colors(true),
            ((69, 69, 69, 255), (0, 0, 0, 90))
        );
        assert_eq!(
            RangeSliderState::handle_colors(false),
            ((255, 255, 255, 255), (0, 0, 0, 25))
        );
        assert_eq!(
            RangeSliderState::label_colors(true),
            ((50, 50, 50, 255), (255, 255, 255, 15), (255, 255, 255))
        );
        assert_eq!(
            RangeSliderState::label_colors(false),
            ((249, 249, 249, 255), (0, 0, 0, 15), (0, 0, 0))
        );
        assert_eq!(
            RangeSliderState::preview_colors(true),
            ((50, 50, 50, 255), (255, 255, 255, 30))
        );
        assert_eq!(
            RangeSliderState::preview_colors(false),
            ((249, 249, 249, 255), (0, 0, 0, 25))
        );
    }
}
