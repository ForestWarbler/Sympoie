//! Calor as a VST3 and CLAP plugin, so it runs in other hosts as it does in Sympoie: the
//! same processor (`sympoie-calor`) and the same graphite interface (`sympoie-calor-ui`),
//! wrapped with nice-plug.
//!
//! Each continuous parameter is shown to the host as its 0–1 position on Calor's own
//! scale, with Calor's own text for its value, so automation sweeps exactly the curves
//! Sympoie's built-in Calor does (the frequencies are logarithmic) and a position means
//! the same setting in either. The band count, slopes, models, channel modes and
//! modulation routings are choices, Auto Gain, Solo and Mute switches, and Bypass the
//! host's own. Every parameter keeps the key it was first saved under.
//! When the host renders offline it reactivates the plugin, which then runs the curve at
//! its offline rate; the latency stays the same.
//!
//! This is the one Calor crate that allows `unsafe`: the export macros write the C entry
//! points hosts look for, and nice-plug's `Params` trait is unsafe to implement by hand.
//! Nothing else here needs it.

use std::sync::Arc;

use nice_plug::context::gui::GuiContext;
use nice_plug::editor::dpi::LogicalSize;
use nice_plug::prelude::*;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, NiceEguiApp, RepaintNotifier, baseview,
};
use sympoie_calor::params::{
    AUTO_GAIN, BANDS, CROSSOVER_BASE, CROSSOVER_SLOPE, CROSSOVERS, ENVELOPE_ATTACK,
    ENVELOPE_RELEASE, LFO_RATE, LFO_SHAPE, MAX_BANDS, MIX, MOD_AMOUNT, MOD_BAND, MOD_SLOTS,
    MOD_SOURCE, MOD_TARGET, OUTPUT, Slot, TONE, band_parameter, band_slot, mod_parameter,
};
use sympoie_calor::{COUNT, Calor, PARAMETERS, Quality, Spec, Telemetry, choices};

/// Host keys of a band's controls after the first band's, in `Slot` order.
macro_rules! band_keys {
    ($band:literal) => {
        [
            concat!("band", $band, "_drive"),
            concat!("band", $band, "_hardness"),
            concat!("band", $band, "_bias"),
            concat!("band", $band, "_odd"),
            concat!("band", $band, "_even"),
            concat!("band", $band, "_emphasis"),
            concat!("band", $band, "_emphasis_frequency"),
            concat!("band", $band, "_mix"),
            concat!("band", $band, "_level"),
            concat!("band", $band, "_dynamics"),
            concat!("band", $band, "_feedback"),
            concat!("band", $band, "_feedback_frequency"),
            concat!("band", $band, "_model"),
            concat!("band", $band, "_channels"),
            concat!("band", $band, "_solo"),
            concat!("band", $band, "_mute"),
        ]
    };
}

/// The first band keeps the keys its first seven controls have always had.
const FIRST_BAND_KEYS: [&str; 16] = [
    "drive",
    "hardness",
    "bias",
    "odd",
    "even",
    "emphasis",
    "emphasis_frequency",
    "band_mix",
    "level",
    "dynamics",
    "feedback",
    "feedback_frequency",
    "model",
    "channels",
    "solo",
    "mute",
];
const BAND_KEYS: [[&str; 16]; MAX_BANDS] = [
    FIRST_BAND_KEYS,
    band_keys!(2),
    band_keys!(3),
    band_keys!(4),
    band_keys!(5),
    band_keys!(6),
];
const CROSSOVER_KEYS: [&str; CROSSOVERS] = [
    "crossover_1",
    "crossover_2",
    "crossover_3",
    "crossover_4",
    "crossover_5",
];
const MOD_KEYS: [[&str; 4]; MOD_SLOTS] = [
    ["mod1_source", "mod1_target", "mod1_band", "mod1_amount"],
    ["mod2_source", "mod2_target", "mod2_band", "mod2_amount"],
    ["mod3_source", "mod3_target", "mod3_band", "mod3_amount"],
    ["mod4_source", "mod4_target", "mod4_band", "mod4_amount"],
];

const fn keys() -> [&'static str; COUNT] {
    let mut keys = [""; COUNT];
    let mut band = 0;
    while band < MAX_BANDS {
        let mut slot = 0;
        while slot < 16 {
            keys[band_parameter(band, Slot::ALL[slot]) as usize] = BAND_KEYS[band][slot];
            slot += 1;
        }
        band += 1;
    }
    keys[TONE as usize] = "tone";
    keys[MIX as usize] = "mix";
    keys[OUTPUT as usize] = "output";
    keys[AUTO_GAIN as usize] = "auto_gain";
    keys[BANDS as usize] = "bands";
    keys[CROSSOVER_SLOPE as usize] = "crossover_slope";
    let mut crossover = 0;
    while crossover < CROSSOVERS {
        keys[(CROSSOVER_BASE + crossover as u32) as usize] = CROSSOVER_KEYS[crossover];
        crossover += 1;
    }
    keys[LFO_RATE as usize] = "lfo_rate";
    keys[LFO_SHAPE as usize] = "lfo_shape";
    keys[ENVELOPE_ATTACK as usize] = "envelope_attack";
    keys[ENVELOPE_RELEASE as usize] = "envelope_release";
    let mut slot = 0;
    while slot < MOD_SLOTS {
        keys[mod_parameter(slot, MOD_SOURCE) as usize] = MOD_KEYS[slot][0];
        keys[mod_parameter(slot, MOD_TARGET) as usize] = MOD_KEYS[slot][1];
        keys[mod_parameter(slot, MOD_BAND) as usize] = MOD_KEYS[slot][2];
        keys[mod_parameter(slot, MOD_AMOUNT) as usize] = MOD_KEYS[slot][3];
        slot += 1;
    }
    keys
}

/// Stable host ids of Calor's parameters, by Calor id. Hosts store automation and presets
/// under these, so they never change, whatever the parameters come to be called; the
/// first eleven are those of the single-band Calor.
const KEYS: [&str; COUNT] = keys();

const BAND_GROUPS: [&str; MAX_BANDS] = ["Band 1", "Band 2", "Band 3", "Band 4", "Band 5", "Band 6"];

/// The group a host files each parameter under in its own parameter lists.
fn group(id: u32) -> &'static str {
    if let Some((band, _)) = band_slot(id) {
        return BAND_GROUPS[band];
    }
    match id {
        TONE | MIX | OUTPUT | AUTO_GAIN => "Output",
        id if id == BANDS || id == CROSSOVER_SLOPE => "Bands",
        id if (CROSSOVER_BASE..CROSSOVER_BASE + CROSSOVERS as u32).contains(&id) => "Bands",
        _ => "Modulation",
    }
}

/// Window size the interface opens at, and the smallest it may be made.
const SIZE: (f32, f32) = (900.0, 500.0);
const MIN_SIZE: (f32, f32) = (630.0, 350.0);

/// One parameter as the host sees it: a continuous control as its 0–1 position on
/// Calor's own scale, a choice as its index, or a switch.
enum HostParam {
    Position(FloatParam),
    Choice(IntParam),
    Switch(BoolParam),
}

impl HostParam {
    fn new(spec: &'static Spec) -> Self {
        if let Some(names) = choices(spec.id) {
            let base = spec.min as i32;
            return Self::Choice(
                IntParam::new(
                    spec.name,
                    spec.default as i32,
                    IntRange::Linear {
                        min: base,
                        max: spec.max as i32,
                    },
                )
                .with_value_to_string(Arc::new(move |value| {
                    names
                        .get((value - base).max(0) as usize)
                        .copied()
                        .unwrap_or_default()
                        .to_owned()
                }))
                .with_string_to_value(Arc::new(move |text| {
                    names
                        .iter()
                        .position(|name| name.eq_ignore_ascii_case(text.trim()))
                        .map(|index| index as i32 + base)
                })),
            );
        }
        if spec.steps == 1 {
            return Self::Switch(BoolParam::new(spec.name, spec.default >= 0.5));
        }
        Self::Position(
            FloatParam::new(
                spec.name,
                spec.normalize(spec.default) as f32,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_value_to_string(Arc::new(move |position| {
                sympoie_calor_ui::format_value(spec, spec.denormalize(f64::from(position)))
            }))
            .with_string_to_value(Arc::new(move |text| {
                sympoie_calor_ui::parse_value(spec, text).map(|value| spec.normalize(value) as f32)
            })),
        )
    }

    fn pointer(&self) -> ParamPtr {
        match self {
            Self::Position(param) => param.as_ptr(),
            Self::Choice(param) => param.as_ptr(),
            Self::Switch(param) => param.as_ptr(),
        }
    }
}

/// Calor's parameters as the host sees them.
pub struct CalorParams {
    /// Every parameter, by Calor id.
    params: Vec<HostParam>,
    bypass: BoolParam,
    editor: Arc<EguiEditorState>,
}

impl Default for CalorParams {
    fn default() -> Self {
        Self {
            params: PARAMETERS.iter().map(HostParam::new).collect(),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            editor: EguiEditorState::from_size(LogicalSize::new(SIZE.0, SIZE.1), 1.0),
        }
    }
}

impl CalorParams {
    /// A parameter's value in Calor's physical units.
    fn physical(&self, id: u32) -> f64 {
        match self.params.get(id as usize) {
            Some(HostParam::Position(param)) => {
                PARAMETERS[id as usize].denormalize(f64::from(param.value()))
            }
            Some(HostParam::Choice(param)) => f64::from(param.value()),
            Some(HostParam::Switch(param)) => f64::from(u8::from(param.value())),
            None => 0.0,
        }
    }
}

// SAFETY: every pointer in the map points at a parameter owned by this object, which the
// host keeps alive in an `Arc` for as long as it uses the map, and no parameter moves: the
// `params` vector is filled once and never changed.
unsafe impl Params for CalorParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map: Vec<(String, ParamPtr, String)> = self
            .params
            .iter()
            .enumerate()
            .map(|(id, param)| {
                (
                    KEYS[id].to_owned(),
                    param.pointer(),
                    group(id as u32).to_owned(),
                )
            })
            .collect();
        map.push(("bypass".to_owned(), self.bypass.as_ptr(), String::new()));
        map
    }
}

/// The value the processor hears for a parameter. Bypassed, it hears Mix at 0 and Output
/// at 0 dB, which passes the input through unchanged after the same delay.
fn heard(params: &CalorParams, id: u32, bypassed: bool) -> f64 {
    match id {
        MIX | OUTPUT if bypassed => 0.0,
        _ => params.physical(id),
    }
}

/// The path Calor runs for the host's processing mode.
fn quality(mode: ProcessMode) -> Quality {
    match mode {
        ProcessMode::Offline => Quality::Offline,
        ProcessMode::Realtime | ProcessMode::Buffered => Quality::Realtime,
    }
}

/// The plugin: its parameters, the processor at the host's rate, and what the processor
/// shows the interface.
pub struct CalorPlugin {
    params: Arc<CalorParams>,
    telemetry: Arc<Telemetry>,
    calor: Option<Calor>,
    /// The values last handed to the processor, so only changes are passed on.
    applied: [f64; COUNT],
    /// The right channel for a mono signal, sized for the host's largest block.
    mono: Vec<f32>,
}

impl Default for CalorPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(CalorParams::default()),
            telemetry: Arc::new(Telemetry::default()),
            calor: None,
            applied: [f64::NAN; COUNT],
            mono: Vec::new(),
        }
    }
}

impl CalorPlugin {
    /// Hands the processor every parameter that changed, as [`heard`] has it, so switching
    /// bypass in and out glides and keeps time.
    fn apply_parameters(&mut self) {
        let Some(calor) = &mut self.calor else {
            return;
        };
        let bypassed = self.params.bypass.value();
        for id in 0..COUNT as u32 {
            let value = heard(&self.params, id, bypassed);
            if self.applied[id as usize] != value {
                calor.set_parameter(id, value);
                self.applied[id as usize] = value;
            }
        }
    }
}

impl Plugin for CalorPlugin {
    const NAME: &'static str = "Calor";
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
    type Editor = EguiEditor<CalorEditor>;

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
                .with_tile("Calor")
                .with_resize_hint(resize),
            CalorEditor {
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
        let mut calor = Calor::new(
            f64::from(buffer_config.sample_rate),
            Some(self.telemetry.clone()),
        );
        calor.set_quality(quality(buffer_config.process_mode));
        context.set_latency_samples(calor.latency());
        self.calor = Some(calor);
        self.applied = [f64::NAN; COUNT];
        self.apply_parameters();
        if let Some(calor) = &mut self.calor {
            calor.reset();
        }
        self.mono = vec![0.0; buffer_config.max_buffer_size as usize];
        true
    }

    fn reset(&mut self) {
        if let Some(calor) = &mut self.calor {
            calor.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        self.apply_parameters();
        let Some(calor) = &mut self.calor else {
            return ProcessStatus::Normal;
        };
        match buffer.as_slice() {
            [left, right, ..] => calor.process_channels(left, right),
            [mono] => {
                // A mono signal runs as a stereo one with both sides the same.
                let length = mono.len().min(self.mono.len());
                let right = &mut self.mono[..length];
                right.copy_from_slice(&mono[..length]);
                calor.process_channels(&mut mono[..length], right);
            }
            [] => {}
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for CalorPlugin {
    const CLAP_ID: &'static str = "com.sympoie.calor";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A harmonic saturator with separate control of the odd and even harmonics");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
        ClapFeature::Stereo,
        ClapFeature::Mono,
    ];
}

impl Vst3Plugin for CalorPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"SympoieCalorFx01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nice_export_clap!(CalorPlugin);
nice_export_vst3!(CalorPlugin);

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
pub struct CalorEditor {
    params: Arc<CalorParams>,
    telemetry: Arc<Telemetry>,
    gui: Option<GuiContext>,
}

impl NiceEguiApp for CalorEditor {
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
        sympoie_calor_ui::panel(
            ui,
            &mut controls,
            Some(&self.telemetry),
            egui::Id::new("calor"),
        );
    }

    fn editor_closed(&mut self) {
        self.gui = None;
    }
}

/// The interface's view of the host's parameters: every change goes through the host as
/// a gesture, so it records automation and undo as it would for its own controls.
struct HostControls<'a> {
    params: &'a CalorParams,
    setter: ParamSetter<'a>,
}

impl sympoie_calor_ui::Controls for HostControls<'_> {
    fn value(&self, id: u32) -> f64 {
        self.params.physical(id)
    }

    fn begin(&mut self, id: u32) {
        match self.params.params.get(id as usize) {
            Some(HostParam::Position(param)) => self.setter.begin_set_parameter(param),
            Some(HostParam::Choice(param)) => self.setter.begin_set_parameter(param),
            Some(HostParam::Switch(param)) => self.setter.begin_set_parameter(param),
            None => {}
        }
    }

    fn set(&mut self, id: u32, value: f64) {
        match self.params.params.get(id as usize) {
            Some(HostParam::Position(param)) => {
                let spec = &PARAMETERS[id as usize];
                self.setter
                    .set_parameter(param, spec.normalize(value) as f32);
            }
            Some(HostParam::Choice(param)) => {
                self.setter.set_parameter(param, value.round() as i32);
            }
            Some(HostParam::Switch(param)) => self.setter.set_parameter(param, value >= 0.5),
            None => {}
        }
    }

    fn end(&mut self, id: u32) {
        match self.params.params.get(id as usize) {
            Some(HostParam::Position(param)) => self.setter.end_set_parameter(param),
            Some(HostParam::Choice(param)) => self.setter.end_set_parameter(param),
            Some(HostParam::Switch(param)) => self.setter.end_set_parameter(param),
            None => {}
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
        let params = CalorParams::default();
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
        // The single-band Calor's keys are where sets saved with it look for them.
        assert_eq!(
            &KEYS[..11],
            &[
                "drive",
                "hardness",
                "bias",
                "odd",
                "even",
                "emphasis",
                "emphasis_frequency",
                "tone",
                "mix",
                "output",
                "auto_gain"
            ]
        );
        assert_eq!(
            KEYS[band_parameter(3, Slot::Feedback) as usize],
            "band4_feedback"
        );
        assert_eq!(KEYS[mod_parameter(2, MOD_TARGET) as usize], "mod3_target");
        // Hosts show Calor's own text for a position or a choice, and read typed values back.
        let HostParam::Position(frequency) =
            &params.params[sympoie_calor::EMPHASIS_FREQUENCY as usize]
        else {
            panic!("the emphasis frequency is continuous");
        };
        assert_eq!(frequency.to_string(), "2.00 kHz");
        let position = frequency.string_to_normalized_value("3.5k").unwrap();
        let spec = &PARAMETERS[sympoie_calor::EMPHASIS_FREQUENCY as usize];
        assert!((spec.denormalize(f64::from(position)) - 3500.0).abs() < 0.5);
        let HostParam::Choice(model) = &params.params[band_parameter(1, Slot::Model) as usize]
        else {
            panic!("a model is a choice");
        };
        assert_eq!(model.to_string(), "Curve");
        let HostParam::Choice(bands) = &params.params[BANDS as usize] else {
            panic!("the band count is a choice");
        };
        assert_eq!(bands.to_string(), "1");
        assert!(
            matches!(&params.params[AUTO_GAIN as usize], HostParam::Switch(param) if param.value())
        );
        assert!(matches!(
            &params.params[band_parameter(4, Slot::Mute) as usize],
            HostParam::Switch(param) if !param.value()
        ));
        assert_eq!(quality(ProcessMode::Offline), Quality::Offline);
        assert_eq!(quality(ProcessMode::Realtime), Quality::Realtime);
    }

    #[test]
    fn bypass_passes_the_input_through_at_unity_after_the_latency() {
        let params = CalorParams::default();
        for id in 0..COUNT as u32 {
            let bypassed = heard(&params, id, true);
            if matches!(id, MIX | OUTPUT) {
                assert_eq!(bypassed, 0.0, "{}", PARAMETERS[id as usize].name);
            } else {
                assert_eq!(bypassed, params.physical(id));
            }
        }
        // What the processor hears then, Output first set well away from unity.
        let mut calor = Calor::new(48_000.0, None);
        calor.set_parameter(OUTPUT, 9.0);
        calor.reset();
        for id in 0..COUNT as u32 {
            calor.set_parameter(id, heard(&params, id, true));
        }
        let lag = calor.latency() as usize;
        let input: Vec<f32> = (0..8192).map(|i| (i as f32 * 0.37).sin() * 0.5).collect();
        let (mut left, mut right) = (input.clone(), input.clone());
        calor.process_channels(&mut left, &mut right);
        // Output and Mix glide back over their 15 ms smoothing; after ten of those they are
        // there.
        for (out, original) in left[lag + 7200..].iter().zip(&input[7200..]) {
            assert!((out - original).abs() < 1.0e-4, "{out} vs {original}");
        }
    }
}
