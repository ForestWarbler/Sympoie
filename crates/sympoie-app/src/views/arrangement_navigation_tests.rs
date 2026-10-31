use super::*;

struct NavigationHarness {
    ctx: egui::Context,
    project: Project,
    state: ArrangementState,
    status: EngineStatus,
    time: f64,
    screen_size: Vec2,
    shapes: Vec<egui::epaint::ClippedShape>,
}

impl NavigationHarness {
    fn new() -> Self {
        let mut harness = Self {
            ctx: egui::Context::default(),
            project: Project::new(),
            state: ArrangementState::default(),
            status: EngineStatus {
                state: PlaybackState::Stopped,
                position_frame: 0,
                render_error: None,
            },
            time: 0.0,
            screen_size: vec2(1000.0, 500.0),
            shapes: Vec::new(),
        };
        harness.step(Vec::new());
        harness.step(Vec::new());
        harness
    }

    fn step(&mut self, events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen_size)),
            time: Some(self.time),
            focused: true,
            events,
            ..Default::default()
        };
        let inputs = Inputs {
            project: &self.project,
            status: &self.status,
            selection: &Selection::default(),
            loop_region: LoopRegion::default(),
            armed_tracks: &BTreeSet::new(),
            input_catalog: &InputCatalog::default(),
            recording: false,
            recording_start_tick: None,
        };
        let mut output = self.ctx.run_ui(input, |ui| {
            show(
                ui,
                &mut self.state,
                &inputs,
                &mut WaveformCache::default(),
                &mut [],
                &mut widgets::MeterState::default(),
                &mut Vec::new(),
            );
        });
        output.textures_delta.clear();
        self.shapes = output.shapes;
    }

    fn overview(&self) -> Rect {
        self.ctx
            .read_response(Id::new("arrangement-overview"))
            .unwrap()
            .rect
    }

    fn thumb(&self) -> Rect {
        overview_thumb(
            self.overview(),
            self.state.scroll_tick,
            self.state.visible_ticks(),
            self.state.overview_end_tick,
        )
    }

    fn position(&mut self, scroll: f64, zoom: f64, total: f64) {
        self.state.follow = false;
        self.state.scroll_tick = scroll;
        self.state.pixels_per_tick = zoom;
        self.state.overview_end_tick = total;
        self.step(Vec::new());
    }

    fn press(&mut self, point: Pos2) {
        self.step(vec![egui::Event::PointerMoved(point)]);
        self.step(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
    }

    fn move_pointer(&mut self, point: Pos2) {
        self.step(vec![egui::Event::PointerMoved(point)]);
    }

    fn release(&mut self, point: Pos2) {
        self.step(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
    }

    fn click(&mut self, point: Pos2) {
        self.press(point);
        self.release(point);
    }
}

fn approximately(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn overview_thumb_retains_both_edges_at_distant_positions_and_extreme_zoom() {
    for width in [12.0, 28.0, 748.0] {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(width, 18.0));
        let visible = f64::from(width) / state::MAX_ZOOM;
        let total = 1_000_000_000.0 + visible;
        for scroll in [0.0, total / 2.0, total - visible] {
            let thumb = overview_thumb(rect, scroll, visible, total);
            assert!(thumb.left() >= rect.left());
            assert!(thumb.right() <= rect.right());
            assert!(thumb.width() >= width.min(28.0) - 0.001);
            assert!(thumb.width() <= rect.width());
        }
        let full = overview_thumb(rect, 0.0, total, total);
        assert_eq!(full.left(), rect.left());
        assert_eq!(full.right(), rect.right());
    }
}

#[test]
fn overview_body_drag_is_bounded_and_keeps_its_extent_after_release() {
    for right in [false, true] {
        let mut harness = NavigationHarness::new();
        harness.position(40_000.0, 0.05, 200_000.0);
        let total = harness.state.overview_end_tick;
        let visible = harness.state.visible_ticks();
        let origin = harness.thumb().center();
        let destination = pos2(if right { 10_000.0 } else { -10_000.0 }, origin.y);
        harness.press(origin);
        harness.move_pointer(origin + vec2(if right { 12.0 } else { -12.0 }, 0.0));
        harness.move_pointer(destination);
        approximately(
            harness.state.scroll_tick,
            if right { total - visible } else { 0.0 },
        );
        approximately(harness.state.overview_end_tick, total);
        let browsed = harness.state.scroll_tick;
        harness.release(destination);
        harness.step(Vec::new());
        approximately(harness.state.scroll_tick, browsed);
        approximately(harness.state.overview_end_tick, total);
        assert!(!harness.state.follow);
    }
}

#[test]
fn overview_pan_updates_the_lane_geometry_in_the_same_frame() {
    let mut harness = NavigationHarness::new();
    harness.state.playback_start = 50_000;
    harness.position(40_000.0, 0.05, 200_000.0);
    let origin = harness.thumb().center();
    harness.press(origin);
    harness.move_pointer(origin + vec2(12.0, 0.0));
    let expected_x = (harness.overview().left()
        + ((harness.state.playback_start as f64 - harness.state.scroll_tick)
            * harness.state.pixels_per_tick) as f32)
        .round()
        + 0.5;
    let marker_x = harness.shapes.iter().find_map(|paint| match &paint.shape {
        Shape::LineSegment { points, stroke }
            if stroke.color == theme::PLAYHEAD && points[1].y - points[0].y > 100.0 =>
        {
            Some(points[0].x)
        }
        _ => None,
    });
    assert_eq!(marker_x, Some(expected_x));
}

#[test]
fn resizing_the_window_cancels_an_overview_drag_without_reinterpreting_pointer_motion() {
    let mut harness = NavigationHarness::new();
    harness.position(40_000.0, 0.05, 200_000.0);
    let origin = harness.thumb().center();
    harness.press(origin);
    harness.move_pointer(origin + vec2(12.0, 0.0));
    let scroll = harness.state.scroll_tick;
    let zoom = harness.state.pixels_per_tick;
    harness.screen_size.x = 820.0;
    harness.step(Vec::new());
    let destination = origin + vec2(300.0, 0.0);
    harness.move_pointer(destination);
    harness.release(destination);
    approximately(harness.state.scroll_tick, scroll);
    approximately(harness.state.pixels_per_tick, zoom);
}

#[test]
fn overview_edge_drag_keeps_the_opposite_tick_fixed() {
    for left in [false, true] {
        let mut harness = NavigationHarness::new();
        harness.position(40_000.0, 0.05, 200_000.0);
        let initial_start = harness.state.scroll_tick;
        let initial_end = initial_start + harness.state.visible_ticks();
        let initial_zoom = harness.state.pixels_per_tick;
        let thumb = harness.thumb();
        let origin = pos2(
            if left {
                thumb.left() + 2.0
            } else {
                thumb.right() - 2.0
            },
            thumb.center().y,
        );
        let destination = origin + vec2(12.0, 0.0);
        harness.press(origin);
        harness.move_pointer(destination);
        harness.release(destination);
        harness.step(Vec::new());
        if left {
            approximately(
                harness.state.scroll_tick + harness.state.visible_ticks(),
                initial_end,
            );
            assert!(harness.state.pixels_per_tick > initial_zoom);
        } else {
            approximately(harness.state.scroll_tick, initial_start);
            assert!(harness.state.pixels_per_tick < initial_zoom);
        }
    }
}

#[test]
fn overview_handles_preserve_the_anchor_at_both_zoom_limits() {
    for left in [false, true] {
        for zoom_in in [false, true] {
            let mut harness = NavigationHarness::new();
            harness.position(3_000_000.0, 0.05, 10_000_000.0);
            let initial_start = harness.state.scroll_tick;
            let initial_end = initial_start + harness.state.visible_ticks();
            let thumb = harness.thumb();
            let origin = pos2(
                if left {
                    thumb.left() + 2.0
                } else {
                    thumb.right() - 2.0
                },
                thumb.center().y,
            );
            let delta = if left == zoom_in {
                1_000_000.0
            } else {
                -1_000_000.0
            };
            let destination = origin + vec2(delta, 0.0);
            harness.press(origin);
            harness.move_pointer(destination);
            harness.release(destination);
            harness.step(Vec::new());
            approximately(
                harness.state.pixels_per_tick,
                if zoom_in {
                    state::MAX_ZOOM
                } else {
                    state::MIN_ZOOM
                },
            );
            if left {
                approximately(
                    harness.state.scroll_tick + harness.state.visible_ticks(),
                    initial_end,
                );
            } else {
                approximately(harness.state.scroll_tick, initial_start);
            }
        }
    }
}

#[test]
fn overview_left_handle_stops_at_song_start_without_moving_the_right_edge() {
    let mut harness = NavigationHarness::new();
    harness.position(5_000.0, 0.05, 200_000.0);
    let initial_end = harness.state.scroll_tick + harness.state.visible_ticks();
    let thumb = harness.thumb();
    let origin = pos2(thumb.left() + 2.0, thumb.center().y);
    let destination = origin - vec2(10_000.0, 0.0);
    harness.press(origin);
    harness.move_pointer(destination);
    harness.release(destination);
    harness.step(Vec::new());
    approximately(harness.state.scroll_tick, 0.0);
    approximately(harness.state.visible_ticks(), initial_end);
}

#[test]
fn overview_double_click_fits_content_and_forgets_distant_navigation_extent() {
    let mut harness = NavigationHarness::new();
    harness.position(10_000_000.0, state::MAX_ZOOM, 20_000_000.0);
    let point = harness.overview().center();
    harness.click(point);
    harness.click(point);
    harness.step(Vec::new());
    approximately(harness.state.scroll_tick, 0.0);
    assert!(harness.state.overview_end_tick < 1_000_000.0);
    assert!(harness.state.visible_ticks() > 10_000.0);
    assert!(!harness.state.follow);
}

#[test]
fn pointer_zoom_preserves_its_tick_and_takes_control_from_playback_follow() {
    let mut harness = NavigationHarness::new();
    harness.position(20_000.0, 0.05, 200_000.0);
    harness.state.follow = true;
    harness.status.state = PlaybackState::Playing;
    harness.status.position_frame = 30_000_000;
    let pointer = harness.overview().left_bottom() + vec2(200.0, 100.0);
    let anchor_x = pointer.x - harness.overview().left();
    let tick_before =
        harness.state.scroll_tick + f64::from(anchor_x) / harness.state.pixels_per_tick;
    harness.step(vec![
        egui::Event::PointerMoved(pointer),
        egui::Event::Zoom(2.0),
    ]);
    let tick_after =
        harness.state.scroll_tick + f64::from(anchor_x) / harness.state.pixels_per_tick;
    approximately(tick_after, tick_before);
    assert!(!harness.state.follow);
    let browsed = harness.state.scroll_tick;
    harness.status.position_frame *= 2;
    harness.step(Vec::new());
    approximately(harness.state.scroll_tick, browsed);
}

#[test]
fn wheel_pan_can_extend_the_timeline_without_shrinking_its_map_on_return_or_zoom() {
    let mut harness = NavigationHarness::new();
    harness.position(100_000.0, 0.05, 200_000.0);
    let initial_extent = harness.state.overview_end_tick;
    let pointer = harness.overview().left_bottom() + vec2(200.0, 100.0);
    let pan = |harness: &mut NavigationHarness, horizontal: f32| {
        harness.step(vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(horizontal, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ]);
        for _ in 0..120 {
            harness.step(Vec::new());
        }
    };
    pan(&mut harness, -100_000.0);
    assert!(harness.state.scroll_tick > initial_extent);
    let expanded = harness.state.overview_end_tick;
    assert!(expanded > harness.state.scroll_tick + harness.state.visible_ticks());
    let distant_scroll = harness.state.scroll_tick;
    pan(&mut harness, 100_000.0);
    assert!(harness.state.scroll_tick < distant_scroll);
    approximately(harness.state.overview_end_tick, expanded);
    harness.step(vec![egui::Event::Zoom(2.0)]);
    approximately(harness.state.overview_end_tick, expanded);
}
