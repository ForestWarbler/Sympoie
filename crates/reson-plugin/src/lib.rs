//! Reson as a VST3 and CLAP plugin, so it runs in other hosts as it does in Sympoie: the
//! same processor (`sympoie-reson`) and the same graphite interface
//! (`sympoie-reson-ui`), wrapped with nice-plug.
//!
//! Each continuous parameter is shown to the host as its 0–1 position on Reson's own scale,
//! with Reson's own text for its value, so automation sweeps exactly the curves Sympoie's
//! built-in Reson does (Width, Attack and the band frequencies are logarithmic) and a
//! position means the same setting in either. Stereo is a three-way choice, Delta a
//! switch, and Bypass the host's own.
//!
//! This is the one crate in the workspace that allows `unsafe`: the export macros write
//! the C entry points hosts look for, and nice-plug's `Params` trait is unsafe to implement
//! by hand. Nothing else here needs it.

use std::sync::Arc;

use nice_plug::context::gui::GuiContext;
use nice_plug::editor::dpi::LogicalSize;
use nice_plug::prelude::*;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, NiceEguiApp, RepaintNotifier, baseview,
};
use sympoie_reson::params::{DELTA, MIX, OUTPUT, STEREO, STEREO_MODES};
use sympoie_reson::{COUNT, PARAMETERS, Reson, Telemetry};

/// Stable host ids of Reson's parameters, by Reson id. Hosts store automation and presets
/// under these, so they never change, whatever the parameters come to be called.
const KEYS: [&str; COUNT] = [
    "depth",
    "width",
    "threshold",
    "attack",
    "release",
    "knee",
    "stereo",
    "mix",
    "output",
    "delta",
    "low_cut",
    "high_cut",
    "low_shelf_frequency",
    "low_shelf_gain",
    "low_shelf_q",
    "bell_1_frequency",
    "bell_1_gain",
    "bell_1_q",
    "bell_2_frequency",
    "bell_2_gain",
    "bell_2_q",
    "bell_3_frequency",
    "bell_3_gain",
    "bell_3_q",
    "high_shelf_frequency",
    "high_shelf_gain",
    "high_shelf_q",
    "focus",
    "clean",
];

/// The group a host files each parameter under in its own parameter lists.
fn group(id: u32) -> &'static str {
    use sympoie_reson::params::*;
    match id {
        DEPTH | THRESHOLD | KNEE | WIDTH => "Detect",
        FOCUS | CLEAN => "Notes",
        ATTACK | RELEASE | STEREO => "Time",
        MIX | OUTPUT | DELTA => "Output",
        _ => "Sensitivity",
    }
}

/// Window size the interface opens at, and the smallest it may be made.
const SIZE: (f32, f32) = (900.0, 500.0);
const MIN_SIZE: (f32, f32) = (630.0, 350.0);

/// Reson's parameters as the host sees them.
pub struct ResonParams {
    /// The continuous parameters, each its 0–1 position on Reson's own scale, by Reson
    /// id. Stereo and Delta have no entry here.
    positions: Vec<Option<FloatParam>>,
    stereo: IntParam,
    delta: BoolParam,
    bypass: BoolParam,
    editor: Arc<EguiEditorState>,
}

impl Default for ResonParams {
    fn default() -> Self {
        let positions = PARAMETERS
            .iter()
            .map(|spec| {
                (spec.id != STEREO && spec.id != DELTA).then(|| {
                    FloatParam::new(
                        spec.name,
                        spec.normalize(spec.default) as f32,
                        FloatRange::Linear { min: 0.0, max: 1.0 },
                    )
                    .with_value_to_string(Arc::new(move |position| {
                        sympoie_reson_ui::format_value(spec, spec.denormalize(f64::from(position)))
                    }))
                    .with_string_to_value(Arc::new(move |text| {
                        sympoie_reson_ui::parse_value(spec, text)
                            .map(|value| spec.normalize(value) as f32)
                    }))
                })
            })
            .collect();
        Self {
            positions,
            stereo: IntParam::new("Stereo", 0, IntRange::Linear { min: 0, max: 2 })
                .with_value_to_string(Arc::new(|mode| {
                    STEREO_MODES
                        .get(mode as usize)
                        .copied()
                        .unwrap_or_default()
                        .to_owned()
                }))
                .with_string_to_value(Arc::new(|text| {
                    STEREO_MODES
                        .iter()
                        .position(|mode| mode.eq_ignore_ascii_case(text.trim()))
                        .map(|mode| mode as i32)
                })),
            delta: BoolParam::new("Delta", false),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            editor: EguiEditorState::from_size(LogicalSize::new(SIZE.0, SIZE.1), 1.0),
        }
    }
}

impl ResonParams {
    /// A parameter's value in Reson's physical units.
    fn physical(&self, id: u32) -> f64 {
        match id {
            STEREO => f64::from(self.stereo.value()),
            DELTA => {
                if self.delta.value() {
                    1.0
                } else {
                    0.0
                }
            }
            _ => match self.positions.get(id as usize) {
                Some(Some(position)) => {
                    PARAMETERS[id as usize].denormalize(f64::from(position.value()))
                }
                _ => 0.0,
            },
        }
    }

    fn position(&self, id: u32) -> Option<&FloatParam> {
        self.positions.get(id as usize).and_then(Option::as_ref)
    }
}

// SAFETY: every pointer in the map points at a parameter owned by this object, which the
// host keeps alive in an `Arc` for as long as it uses the map, and no parameter moves: the
// `positions` vector is filled once and never changed.
unsafe impl Params for ResonParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map: Vec<(String, ParamPtr, String)> = KEYS
            .iter()
            .enumerate()
            .filter_map(|(id, key)| {
                let id = id as u32;
                let pointer = match id {
                    STEREO => self.stereo.as_ptr(),
                    DELTA => self.delta.as_ptr(),
                    _ => self.position(id)?.as_ptr(),
                };
                Some((key.to_string(), pointer, group(id).to_owned()))
            })
            .collect();
        map.push(("bypass".to_owned(), self.bypass.as_ptr(), String::new()));
        map
    }
}

/// The value the processor hears for a parameter. Bypassed, it hears Mix at 0, Delta off
/// and Output at 0 dB, which passes the input through unchanged after the same delay.
fn heard(params: &ResonParams, id: u32, bypassed: bool) -> f64 {
    match id {
        MIX | DELTA | OUTPUT if bypassed => 0.0,
        _ => params.physical(id),
    }
}

/// The plugin: its parameters, the processor at the host's rate, and what the processor
/// shows the interface.
pub struct ResonPlugin {
    params: Arc<ResonParams>,
    telemetry: Arc<Telemetry>,
    reson: Option<Reson>,
    /// The values last handed to the processor, so only changes are passed on.
    applied: [f64; COUNT],
    /// The right channel for a mono signal, sized for the host's largest block.
    mono: Vec<f32>,
}

impl Default for ResonPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(ResonParams::default()),
            telemetry: Arc::new(Telemetry::default()),
            reson: None,
            applied: [f64::NAN; COUNT],
            mono: Vec::new(),
        }
    }
}

impl ResonPlugin {
    /// Hands the processor every parameter that changed, as [`heard`] has it, so switching
    /// bypass in and out crossfades and keeps time.
    fn apply_parameters(&mut self) {
        let Some(reson) = &mut self.reson else {
            return;
        };
        let bypassed = self.params.bypass.value();
        for id in 0..COUNT as u32 {
            let value = heard(&self.params, id, bypassed);
            if self.applied[id as usize] != value {
                reson.set_parameter(id, value);
                self.applied[id as usize] = value;
            }
        }
    }
}

impl Plugin for ResonPlugin {
    const NAME: &'static str = "Reson";
    const VENDOR: &'static str = "Sympoie";
    const URL: &'static str = env!("CARGO_PKG_REPOSITORY");
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    type SysExMessage = ();
    type BackgroundTask = ();
    type Editor = EguiEditor<ResonEditor>;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        let resize = ResizeHint {
            preserve_aspect_ratio: true,
            aspect_ratio_width: SIZE.0 as u32,
            aspect_ratio_height: SIZE.1 as u32,
            ..ResizeHint::RESIZABLE
        }
        .with_min_logical_size(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1));
        nice_plug_egui::create_egui_editor(
            self.params.editor.clone(),
            RepaintNotifier::new(),
            EguiNiceSettings::new()
                .with_tile("Reson")
                .with_resize_hint(resize),
            ResonEditor {
                params: self.params.clone(),
                telemetry: self.telemetry.clone(),
                gui: None,
            },
        )
    }

    fn activate(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        let reson = Reson::new(
            f64::from(buffer_config.sample_rate),
            Some(self.telemetry.clone()),
        );
        context.set_latency_samples(reson.latency());
        self.reson = Some(reson);
        self.applied = [f64::NAN; COUNT];
        self.apply_parameters();
        if let Some(reson) = &mut self.reson {
            reson.reset();
        }
        self.mono = vec![0.0; buffer_config.max_buffer_size as usize];
        true
    }

    fn reset(&mut self) {
        if let Some(reson) = &mut self.reson {
            reson.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        self.apply_parameters();
        let Some(reson) = &mut self.reson else {
            return ProcessStatus::Normal;
        };
        match buffer.as_slice() {
            [left, right, ..] => reson.process_channels(left, right),
            [mono] => {
                // A mono signal runs as a stereo one with both sides the same.
                let length = mono.len().min(self.mono.len());
                let right = &mut self.mono[..length];
                right.copy_from_slice(&mono[..length]);
                reson.process_channels(&mut mono[..length], right);
            }
            [] => {}
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for ResonPlugin {
    const CLAP_ID: &'static str = "com.sympoie.reson";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A dynamic resonance suppressor that leaves the notes alone");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Equalizer,
        ClapFeature::Restoration,
        ClapFeature::Stereo,
        ClapFeature::Mono,
    ];
}

impl Vst3Plugin for ResonPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"SympoieResonFx01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Eq,
        Vst3SubCategory::Restoration,
    ];
}

nice_export_clap!(ResonPlugin);
nice_export_vst3!(ResonPlugin);

/// Inter for text and JetBrains Mono for values, the faces Sympoie uses, so the interface
/// looks the same in any host.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, family, bytes) in [
        (
            "Inter Medium",
            egui::FontFamily::Proportional,
            include_bytes!("../../sympoie-app/assets/fonts/Inter-Medium.ttf").as_slice(),
        ),
        (
            "JetBrains Mono Medium",
            egui::FontFamily::Monospace,
            include_bytes!("../../sympoie-app/assets/fonts/JetBrainsMono-Medium.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_static(bytes).into());
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.to_owned());
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
}

/// The plugin window: the shared panel, wired to the host's parameters.
pub struct ResonEditor {
    params: Arc<ResonParams>,
    telemetry: Arc<Telemetry>,
    gui: Option<GuiContext>,
}

impl NiceEguiApp for ResonEditor {
    fn build(
        &mut self,
        egui_ctx: egui::Context,
        nice_gui_ctx: GuiContext,
        _frame: &mut nice_plug_egui::Frame,
    ) -> Result<(), baseview::HandlerError> {
        install_fonts(&egui_ctx);
        self.gui = Some(nice_gui_ctx);
        Ok(())
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut nice_plug_egui::Frame) {
        let Some(gui) = &self.gui else {
            return;
        };
        let mut controls = HostControls {
            params: &self.params,
            setter: gui.param_setter(),
        };
        sympoie_reson_ui::panel(
            ui,
            &mut controls,
            Some(&self.telemetry),
            egui::Id::new("reson"),
        );
    }

    fn editor_closed(&mut self) {
        self.gui = None;
    }
}

/// The interface's view of the host's parameters: every change goes through the host as
/// a gesture, so it records automation and undo as it would for its own controls.
struct HostControls<'a> {
    params: &'a ResonParams,
    setter: ParamSetter<'a>,
}

impl sympoie_reson_ui::Controls for HostControls<'_> {
    fn value(&self, id: u32) -> f64 {
        self.params.physical(id)
    }

    fn begin(&mut self, id: u32) {
        match id {
            STEREO => self.setter.begin_set_parameter(&self.params.stereo),
            DELTA => self.setter.begin_set_parameter(&self.params.delta),
            _ => {
                if let Some(position) = self.params.position(id) {
                    self.setter.begin_set_parameter(position);
                }
            }
        }
    }

    fn set(&mut self, id: u32, value: f64) {
        match id {
            STEREO => self
                .setter
                .set_parameter(&self.params.stereo, value.round().clamp(0.0, 2.0) as i32),
            DELTA => self.setter.set_parameter(&self.params.delta, value >= 0.5),
            _ => {
                if let Some(position) = self.params.position(id) {
                    let spec = &PARAMETERS[id as usize];
                    self.setter
                        .set_parameter(position, spec.normalize(value) as f32);
                }
            }
        }
    }

    fn end(&mut self, id: u32) {
        match id {
            STEREO => self.setter.end_set_parameter(&self.params.stereo),
            DELTA => self.setter.end_set_parameter(&self.params.delta),
            _ => {
                if let Some(position) = self.params.position(id) {
                    self.setter.end_set_parameter(position);
                }
            }
        }
    }

    fn bypassed(&self) -> bool {
        self.params.bypass.value()
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.setter.begin_set_parameter(&self.params.bypass);
        self.setter.set_parameter(&self.params.bypass, bypassed);
        self.setter.end_set_parameter(&self.params.bypass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_has_one_stable_key_and_reads_back_its_value() {
        let params = ResonParams::default();
        let map = params.param_map();
        assert_eq!(map.len(), COUNT + 1);
        let mut keys: Vec<&str> = map.iter().map(|(key, _, _)| key.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), COUNT + 1);
        for spec in PARAMETERS {
            let value = params.physical(spec.id);
            assert!(
                (value - spec.default).abs() <= 1.0e-4 * spec.default.abs().max(1.0),
                "{}: {value} vs {}",
                spec.name,
                spec.default
            );
        }
        // Hosts show Reson's own text for a position, and read typed values back.
        let width = params.position(sympoie_reson::WIDTH).unwrap();
        assert_eq!(width.to_string(), "1/12 oct");
        let position = width.string_to_normalized_value("1/16").unwrap();
        let spec = &PARAMETERS[sympoie_reson::WIDTH as usize];
        assert!((spec.denormalize(f64::from(position)) - 1.0 / 16.0).abs() < 1.0e-6);
        assert_eq!(params.stereo.to_string(), "Link");
    }

    #[test]
    fn bypass_passes_the_input_through_at_unity_after_the_latency() {
        let params = ResonParams::default();
        for id in 0..COUNT as u32 {
            let bypassed = heard(&params, id, true);
            if matches!(id, MIX | DELTA | OUTPUT) {
                assert_eq!(bypassed, 0.0, "{}", PARAMETERS[id as usize].name);
            } else {
                assert_eq!(bypassed, params.physical(id));
            }
        }
        // What the processor hears then, Output first set well away from unity.
        let mut reson = Reson::new(48_000.0, None);
        reson.set_parameter(OUTPUT, 9.0);
        reson.reset();
        for id in 0..COUNT as u32 {
            reson.set_parameter(id, heard(&params, id, true));
        }
        let lag = reson.latency() as usize;
        let input: Vec<f32> = (0..8192).map(|i| (i as f32 * 0.37).sin() * 0.5).collect();
        let (mut left, mut right) = (input.clone(), input.clone());
        reson.process_channels(&mut left, &mut right);
        // Output glides back to unity over its 10 ms smoothing; after ten of those it is
        // there.
        for (out, original) in left[lag + 4800..].iter().zip(&input[4800..]) {
            assert!((out - original).abs() < 1.0e-4, "{out} vs {original}");
        }
    }
}
