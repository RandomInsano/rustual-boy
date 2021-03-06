mod mem_map;

use sinks::*;

use self::mem_map::*;

// Docs claim the sample rate is 41.7khz, but my calculations indicate it should be 41666.66hz repeating
//  (see SAMPLE_CLOCK_PERIOD calculation below), so we take the nearest whole-number sample rate to that.
//  Note that the documentation rounds values in a lot of places, so that's probably what happened here.
pub const SAMPLE_RATE: u32 = 41667;

// 20mhz / 41.7khz = ~480 clocks
const SAMPLE_CLOCK_PERIOD: u32 = 480;

// 20mhz / 260.4hz = ~76805 clocks
const DURATION_CLOCK_PERIOD: u32 = 76805;

// 20mhz / 65.1hz = ~307218 clocks
const ENVELOPE_CLOCK_PERIOD: u32 = 307218;

// 20mhz / 5mhz = 4 clocks
const FREQUENCY_CLOCK_PERIOD: u32 = 4;

// 20mhz / 1041.6hz = ~19200 clocks
const SWEEP_MOD_SMALL_PERIOD: u32 = 19200;

// 20mhz / 130.2hz = ~153600 clocks
const SWEEP_MOD_LARGE_PERIOD: u32 = 153600;

// 20mhz / 500khz = 40 clocks
const NOISE_CLOCK_PERIOD: u32 = 40;

const NUM_WAVE_TABLE_WORDS: u32 = 32;
const NUM_WAVE_TABLES: u32 = 5;
const TOTAL_WAVE_TABLE_SIZE: u32 = NUM_WAVE_TABLE_WORDS * NUM_WAVE_TABLES;

const NUM_MOD_TABLE_WORDS: u32 = 32;

#[derive(Default)]
struct PlayControlReg {
    enable: bool,
    use_duration: bool,
    duration: u32,

    duration_counter: u32,
}

impl PlayControlReg {
    fn write(&mut self, value: u8) {
        self.enable = (value & 0x80) != 0;
        self.use_duration = (value & 0x20) != 0;
        self.duration = (value & 0x1f) as _;

        if self.use_duration {
            self.duration_counter = 0;
        }
    }

    fn duration_clock(&mut self) {
        if self.enable && self.use_duration {
            self.duration_counter += 1;
            if self.duration_counter > self.duration {
                self.enable = false;
            }
        }
    }
}

#[derive(Default)]
struct VolumeReg {
    left: u32,
    right: u32,
}

impl VolumeReg {
    fn write(&mut self, value: u8) {
        self.left = (value >> 4) as _;
        self.right = (value & 0x0f) as _;
    }
}

#[derive(Default)]
struct Envelope {
    reg_data_reload: u32,
    reg_data_direction: bool,
    reg_data_step_interval: u32,

    reg_control_repeat: bool,
    reg_control_enable: bool,

    level: u32,

    envelope_counter: u32,
}

impl Envelope {
    fn write_data_reg(&mut self, value: u8) {
        self.reg_data_reload = (value >> 4) as _;
        self.reg_data_direction = (value & 0x08) != 0;
        self.reg_data_step_interval = (value & 0x07) as _;

        self.level = self.reg_data_reload;
    }

    fn write_control_reg(&mut self, value: u8) {
        self.reg_control_repeat = (value & 0x02) != 0;
        self.reg_control_enable = (value & 0x01) != 0;
    }

    fn level(&self) -> u32 {
        self.level
    }

    fn envelope_clock(&mut self) {
        if self.reg_control_enable {
            self.envelope_counter += 1;
            if self.envelope_counter > self.reg_data_step_interval {
                self.envelope_counter = 0;

                if self.reg_data_direction && self.level < 15 {
                    self.level += 1;
                } else if !self.reg_data_direction && self.level > 0 {
                    self.level -= 1;
                } else if self.reg_control_repeat {
                    self.level = self.reg_data_reload;
                }
            }
        }
    }
}

trait Voice {
    fn reg_play_control(&self) -> &PlayControlReg;
    fn reg_volume(&self) -> &VolumeReg;
    fn envelope(&self) -> &Envelope;
}

#[derive(Default)]
struct StandardVoice {
    reg_play_control: PlayControlReg,

    reg_volume: VolumeReg,

    reg_frequency_low: u32,
    reg_frequency_high: u32,

    envelope: Envelope,

    reg_pcm_wave: u32,

    frequency_counter: u32,
    phase: u32,
}

impl StandardVoice {
    fn write_play_control_reg(&mut self, value: u8) {
        self.reg_play_control.write(value);

        if self.reg_play_control.enable {
            self.envelope.envelope_counter = 0;

            self.frequency_counter = 0;
            self.phase = 0;
        }
    }

    fn write_volume_reg(&mut self, value: u8) {
        self.reg_volume.write(value);
    }

    fn write_frequency_low_reg(&mut self, value: u8) {
        self.reg_frequency_low = value as _;
    }

    fn write_frequency_high_reg(&mut self, value: u8) {
        self.reg_frequency_high = (value & 0x07) as _;
    }

    fn write_envelope_data_reg(&mut self, value: u8) {
        self.envelope.write_data_reg(value);
    }

    fn write_envelope_control_reg(&mut self, value: u8) {
        self.envelope.write_control_reg(value);
    }

    fn write_pcm_wave_reg(&mut self, value: u8) {
        self.reg_pcm_wave = (value & 0x07) as _;
    }

    fn frequency_clock(&mut self) {
        self.frequency_counter += 1;
        if self.frequency_counter >= 2048 - ((self.reg_frequency_high << 8) | self.reg_frequency_low) {
            self.frequency_counter = 0;

            self.phase = (self.phase + 1) & (NUM_WAVE_TABLE_WORDS - 1);
        }
    }

    fn output(&self, wave_tables: &[u8]) -> u32 {
        if self.reg_pcm_wave > 4 {
            return 0;
        }

        wave_tables[(self.reg_pcm_wave * NUM_WAVE_TABLE_WORDS + self.phase) as usize] as _
    }
}

impl Voice for StandardVoice {
    fn reg_play_control(&self) -> &PlayControlReg {
        &self.reg_play_control
    }

    fn reg_volume(&self) -> &VolumeReg {
        &self.reg_volume
    }

    fn envelope(&self) -> &Envelope {
        &self.envelope
    }
}

#[derive(Default)]
struct SweepModVoice {
    reg_play_control: PlayControlReg,

    reg_volume: VolumeReg,

    reg_frequency_low: u32,
    reg_frequency_high: u32,
    frequency_low: u32,
    frequency_high: u32,
    next_frequency_low: u32,
    next_frequency_high: u32,

    envelope: Envelope,

    reg_sweep_mod_enable: bool,
    reg_mod_repeat: bool,
    reg_function: bool,

    reg_sweep_mod_base_interval: bool,
    reg_sweep_mod_interval: u32,
    reg_sweep_direction: bool,
    reg_sweep_shift_amount: u32,

    reg_pcm_wave: u32,

    frequency_counter: u32,
    phase: u32,

    sweep_mod_counter: u32,
    mod_phase: u32,
}

impl SweepModVoice {
    fn write_play_control_reg(&mut self, value: u8) {
        self.reg_play_control.write(value);

        if self.reg_play_control.enable {
            self.envelope.envelope_counter = 0;

            self.frequency_counter = 0;
            self.phase = 0;
            self.sweep_mod_counter = 0;
            self.mod_phase = 0;
        }
    }

    fn write_volume_reg(&mut self, value: u8) {
        self.reg_volume.write(value);
    }

    fn write_frequency_low_reg(&mut self, value: u8) {
        self.reg_frequency_low = value as _;
        self.next_frequency_low = self.reg_frequency_low;
    }

    fn write_frequency_high_reg(&mut self, value: u8) {
        self.reg_frequency_high = (value & 0x07) as _;
        self.next_frequency_high = self.reg_frequency_high;
    }

    fn write_envelope_data_reg(&mut self, value: u8) {
        self.envelope.write_data_reg(value);
    }

    fn write_envelope_sweep_mod_control_reg(&mut self, value: u8) {
        self.envelope.write_control_reg(value);
        self.reg_sweep_mod_enable = ((value >> 6) & 0x01) != 0;
        self.reg_mod_repeat = ((value >> 5) & 0x01) != 0;
        self.reg_function = ((value >> 4) & 0x01) != 0;
    }

    fn write_sweep_mod_data_reg(&mut self, value: u8) {
        self.reg_sweep_mod_base_interval = ((value >> 7) & 0x01) != 0;
        self.reg_sweep_mod_interval = ((value >> 4) & 0x07) as _;
        self.reg_sweep_direction = ((value >> 3) & 0x01) != 0;
        self.reg_sweep_shift_amount = (value & 0x07) as _;
    }

    fn write_pcm_wave_reg(&mut self, value: u8) {
        self.reg_pcm_wave = (value & 0x07) as _;
    }

    fn frequency_clock(&mut self) {
        self.frequency_counter += 1;
        if self.frequency_counter >= 2048 - ((self.frequency_high << 8) | self.frequency_low) {
            self.frequency_counter = 0;

            self.phase = (self.phase + 1) & (NUM_WAVE_TABLE_WORDS - 1);
        }
    }

    fn sweep_mod_clock(&mut self, mod_table: &[i8]) {
        self.sweep_mod_counter += 1;
        if self.sweep_mod_counter >= self.reg_sweep_mod_interval {
            self.sweep_mod_counter = 0;

            self.frequency_low = self.next_frequency_low;
            self.frequency_high = self.next_frequency_high;

            let mut freq = (self.frequency_high << 8) | self.frequency_low;

            if freq >= 2048 {
                self.reg_play_control.enable = false;
            }

            if !self.reg_play_control.enable || !self.reg_sweep_mod_enable || self.reg_sweep_mod_interval == 0 {
                return;
            }

            match self.reg_function {
                false => {
                    // Sweep
                    let sweep_value = freq >> self.reg_sweep_shift_amount;
                    freq = match self.reg_sweep_direction {
                        false => freq.wrapping_sub(sweep_value),
                        true => freq.wrapping_add(sweep_value)
                    };
                }
                true => {
                    // Mod
                    let reg_freq = (self.reg_frequency_high << 8) | self.reg_frequency_low;
                    freq = reg_freq.wrapping_add(mod_table[self.mod_phase as usize] as _) & 0x07ff;

                    const MAX_MOD_PHASE: u32 = NUM_MOD_TABLE_WORDS - 1;
                    self.mod_phase = match (self.reg_mod_repeat, self.mod_phase) {
                        (false, MAX_MOD_PHASE) => MAX_MOD_PHASE,
                        _ => (self.mod_phase + 1) & MAX_MOD_PHASE
                    };
                }
            }

            self.next_frequency_low = freq & 0xff;
            self.next_frequency_high = (freq >> 8) & 0x07;
        }
    }

    fn output(&self, wave_tables: &[u8]) -> u32 {
        if self.reg_pcm_wave > 4 {
            return 0;
        }

        wave_tables[(self.reg_pcm_wave * NUM_WAVE_TABLE_WORDS + self.phase) as usize] as _
    }
}

impl Voice for SweepModVoice {
    fn reg_play_control(&self) -> &PlayControlReg {
        &self.reg_play_control
    }

    fn reg_volume(&self) -> &VolumeReg {
        &self.reg_volume
    }

    fn envelope(&self) -> &Envelope {
        &self.envelope
    }
}

#[derive(Default)]
struct NoiseVoice {
    reg_play_control: PlayControlReg,

    reg_volume: VolumeReg,

    reg_frequency_low: u32,
    reg_frequency_high: u32,

    envelope: Envelope,

    reg_noise_control: u32,

    frequency_counter: u32,
    shift: u32,
    output: u32,
}

impl NoiseVoice {
    fn write_play_control_reg(&mut self, value: u8) {
        self.reg_play_control.write(value);

        if self.reg_play_control.enable {
            self.envelope.envelope_counter = 0;

            self.frequency_counter = 0;
            self.shift = 0x7fff;
        }
    }

    fn write_volume_reg(&mut self, value: u8) {
        self.reg_volume.write(value);
    }

    fn write_frequency_low_reg(&mut self, value: u8) {
        self.reg_frequency_low = value as _;
    }

    fn write_frequency_high_reg(&mut self, value: u8) {
        self.reg_frequency_high = (value & 0x07) as _;
    }

    fn write_envelope_data_reg(&mut self, value: u8) {
        self.envelope.write_data_reg(value);
    }

    fn write_envelope_noise_control_reg(&mut self, value: u8) {
        self.reg_noise_control = ((value >> 4) & 0x07) as _;
        self.envelope.write_control_reg(value);
    }

    fn noise_clock(&mut self) {
        self.frequency_counter += 1;
        if self.frequency_counter >= 2048 - ((self.reg_frequency_high << 8) | self.reg_frequency_low) {
            self.frequency_counter = 0;

            let lhs = self.shift >> 7;

            let rhs_bit_index = match self.reg_noise_control {
                0 => 14,
                1 => 10,
                2 => 13,
                3 => 4,
                4 => 8,
                5 => 6,
                6 => 9,
                _ => 11
            };
            let rhs = self.shift >> rhs_bit_index;

            let xor_bit = (lhs ^ rhs) & 0x01;

            self.shift = ((self.shift << 1) | xor_bit) & 0x7fff;

            let output_bit = (!xor_bit) & 0x01;
            self.output = match output_bit {
                0 => 0,
                _ => 0x3f
            };
        }
    }

    fn output(&self) -> u32 {
        self.output
    }
}

impl Voice for NoiseVoice {
    fn reg_play_control(&self) -> &PlayControlReg {
        &self.reg_play_control
    }

    fn reg_volume(&self) -> &VolumeReg {
        &self.reg_volume
    }

    fn envelope(&self) -> &Envelope {
        &self.envelope
    }
}

pub struct Vsu {
    wave_tables: Box<[u8]>,
    mod_table: Box<[i8]>,

    voice1: StandardVoice,
    voice2: StandardVoice,
    voice3: StandardVoice,
    voice4: StandardVoice,
    voice5: SweepModVoice,
    voice6: NoiseVoice,

    duration_clock_counter: u32,
    envelope_clock_counter: u32,
    frequency_clock_counter: u32,
    sweep_mod_clock_counter: u32,
    noise_clock_counter: u32,
    sample_clock_counter: u32,
}

impl Vsu {
    pub fn new() -> Vsu {
        Vsu {
            wave_tables: vec![0; TOTAL_WAVE_TABLE_SIZE as usize].into_boxed_slice(),
            mod_table: vec![0; NUM_MOD_TABLE_WORDS as usize].into_boxed_slice(),

            voice1: StandardVoice::default(),
            voice2: StandardVoice::default(),
            voice3: StandardVoice::default(),
            voice4: StandardVoice::default(),
            voice5: SweepModVoice::default(),
            voice6: NoiseVoice::default(),

            duration_clock_counter: 0,
            envelope_clock_counter: 0,
            frequency_clock_counter: 0,
            sweep_mod_clock_counter: 0,
            noise_clock_counter: 0,
            sample_clock_counter: 0,
        }
    }

    pub fn read_byte(&self, addr: u32) -> u8 {
        logln!(Log::Vsu, "WARNING: Attempted read byte from VSU (addr: 0x{:08x})", addr);

        0
    }

    pub fn write_byte(&mut self, addr: u32, value: u8) {
        match addr {
            PCM_WAVE_TABLE_0_START ... PCM_WAVE_TABLE_0_END => {
                if !self.are_channels_active() {
                    self.wave_tables[((addr - PCM_WAVE_TABLE_0_START) / 4 + 0x00) as usize] = value & 0x3f;
                }
            }
            PCM_WAVE_TABLE_1_START ... PCM_WAVE_TABLE_1_END => {
                if !self.are_channels_active() {
                    self.wave_tables[((addr - PCM_WAVE_TABLE_1_START) / 4 + 0x20) as usize] = value & 0x3f;
                }
            }
            PCM_WAVE_TABLE_2_START ... PCM_WAVE_TABLE_2_END => {
                if !self.are_channels_active() {
                    self.wave_tables[((addr - PCM_WAVE_TABLE_2_START) / 4 + 0x40) as usize] = value & 0x3f;
                }
            }
            PCM_WAVE_TABLE_3_START ... PCM_WAVE_TABLE_3_END => {
                if !self.are_channels_active() {
                    self.wave_tables[((addr - PCM_WAVE_TABLE_3_START) / 4 + 0x60) as usize] = value & 0x3f;
                }
            }
            PCM_WAVE_TABLE_4_START ... PCM_WAVE_TABLE_4_END => {
                if !self.are_channels_active() {
                    self.wave_tables[((addr - PCM_WAVE_TABLE_4_START) / 4 + 0x80) as usize] = value & 0x3f;
                }
            }
            MOD_TABLE_START ... MOD_TABLE_END => {
                if !self.voice5.reg_play_control.enable {
                    self.mod_table[((addr - MOD_TABLE_START) / 4) as usize] = value as _;
                }
            }
            VOICE_1_PLAY_CONTROL => self.voice1.write_play_control_reg(value),
            VOICE_1_VOLUME => self.voice1.write_volume_reg(value),
            VOICE_1_FREQUENCY_LOW => self.voice1.write_frequency_low_reg(value),
            VOICE_1_FREQUENCY_HIGH => self.voice1.write_frequency_high_reg(value),
            VOICE_1_ENVELOPE_DATA => self.voice1.write_envelope_data_reg(value),
            VOICE_1_ENVELOPE_CONTROL => self.voice1.write_envelope_control_reg(value),
            VOICE_1_PCM_WAVE => self.voice1.write_pcm_wave_reg(value),
            VOICE_2_PLAY_CONTROL => self.voice2.write_play_control_reg(value),
            VOICE_2_VOLUME => self.voice2.write_volume_reg(value),
            VOICE_2_FREQUENCY_LOW => self.voice2.write_frequency_low_reg(value),
            VOICE_2_FREQUENCY_HIGH => self.voice2.write_frequency_high_reg(value),
            VOICE_2_ENVELOPE_DATA => self.voice2.write_envelope_data_reg(value),
            VOICE_2_ENVELOPE_CONTROL => self.voice2.write_envelope_control_reg(value),
            VOICE_2_PCM_WAVE => self.voice2.write_pcm_wave_reg(value),
            VOICE_3_PLAY_CONTROL => self.voice3.write_play_control_reg(value),
            VOICE_3_VOLUME => self.voice3.write_volume_reg(value),
            VOICE_3_FREQUENCY_LOW => self.voice3.write_frequency_low_reg(value),
            VOICE_3_FREQUENCY_HIGH => self.voice3.write_frequency_high_reg(value),
            VOICE_3_ENVELOPE_DATA => self.voice3.write_envelope_data_reg(value),
            VOICE_3_ENVELOPE_CONTROL => self.voice3.write_envelope_control_reg(value),
            VOICE_3_PCM_WAVE => self.voice3.write_pcm_wave_reg(value),
            VOICE_4_PLAY_CONTROL => self.voice4.write_play_control_reg(value),
            VOICE_4_VOLUME => self.voice4.write_volume_reg(value),
            VOICE_4_FREQUENCY_LOW => self.voice4.write_frequency_low_reg(value),
            VOICE_4_FREQUENCY_HIGH => self.voice4.write_frequency_high_reg(value),
            VOICE_4_ENVELOPE_DATA => self.voice4.write_envelope_data_reg(value),
            VOICE_4_ENVELOPE_CONTROL => self.voice4.write_envelope_control_reg(value),
            VOICE_4_PCM_WAVE => self.voice4.write_pcm_wave_reg(value),
            VOICE_5_PLAY_CONTROL => self.voice5.write_play_control_reg(value),
            VOICE_5_VOLUME => self.voice5.write_volume_reg(value),
            VOICE_5_FREQUENCY_LOW => self.voice5.write_frequency_low_reg(value),
            VOICE_5_FREQUENCY_HIGH => self.voice5.write_frequency_high_reg(value),
            VOICE_5_ENVELOPE_DATA => self.voice5.write_envelope_data_reg(value),
            VOICE_5_ENVELOPE_SWEEP_MOD_CONTROL => self.voice5.write_envelope_sweep_mod_control_reg(value),
            VOICE_5_SWEEP_MOD_DATA => self.voice5.write_sweep_mod_data_reg(value),
            VOICE_5_PCM_WAVE => self.voice5.write_pcm_wave_reg(value),
            VOICE_6_PLAY_CONTROL => self.voice6.write_play_control_reg(value),
            VOICE_6_VOLUME => self.voice6.write_volume_reg(value),
            VOICE_6_FREQUENCY_LOW => self.voice6.write_frequency_low_reg(value),
            VOICE_6_FREQUENCY_HIGH => self.voice6.write_frequency_high_reg(value),
            VOICE_6_ENVELOPE_DATA => self.voice6.write_envelope_data_reg(value),
            VOICE_6_ENVELOPE_NOISE_CONTROL => self.voice6.write_envelope_noise_control_reg(value),
            SOUND_DISABLE_REG => {
                if (value & 0x01) != 0 {
                    self.voice1.reg_play_control.enable = false;
                    self.voice2.reg_play_control.enable = false;
                    self.voice3.reg_play_control.enable = false;
                    self.voice4.reg_play_control.enable = false;
                    self.voice5.reg_play_control.enable = false;
                    self.voice6.reg_play_control.enable = false;
                }
            }
            _ => logln!(Log::Vsu, "VSU write byte not yet implemented (addr: 0x{:08x}, value: 0x{:04x})", addr, value)
        }
    }

    pub fn read_halfword(&self, addr: u32) -> u16 {
        logln!(Log::Vsu, "WARNING: Attempted read halfword from VSU (addr: 0x{:08x})", addr);

        0
    }

    pub fn write_halfword(&mut self, addr: u32, value: u16) {
        let addr = addr & 0xfffffffe;
        self.write_byte(addr, value as _);
    }

    pub fn cycles(&mut self, num_cycles: u32, audio_frame_sink: &mut Sink<AudioFrame>) {
        for _ in 0..num_cycles {
            self.duration_clock_counter += 1;
            if self.duration_clock_counter >= DURATION_CLOCK_PERIOD {
                self.duration_clock_counter = 0;

                self.voice1.reg_play_control.duration_clock();
                self.voice2.reg_play_control.duration_clock();
                self.voice3.reg_play_control.duration_clock();
                self.voice4.reg_play_control.duration_clock();
                self.voice5.reg_play_control.duration_clock();
                self.voice6.reg_play_control.duration_clock();
            }

            self.envelope_clock_counter += 1;
            if self.envelope_clock_counter >= ENVELOPE_CLOCK_PERIOD {
                self.envelope_clock_counter = 0;

                self.voice1.envelope.envelope_clock();
                self.voice2.envelope.envelope_clock();
                self.voice3.envelope.envelope_clock();
                self.voice4.envelope.envelope_clock();
                self.voice5.envelope.envelope_clock();
                self.voice6.envelope.envelope_clock();
            }

            self.frequency_clock_counter += 1;
            if self.frequency_clock_counter >= FREQUENCY_CLOCK_PERIOD {
                self.frequency_clock_counter = 0;

                self.voice1.frequency_clock();
                self.voice2.frequency_clock();
                self.voice3.frequency_clock();
                self.voice4.frequency_clock();
                self.voice5.frequency_clock();
            }

            self.sweep_mod_clock_counter += 1;
            let sweep_mod_clock_period = match self.voice5.reg_sweep_mod_base_interval {
                false => SWEEP_MOD_SMALL_PERIOD,
                true => SWEEP_MOD_LARGE_PERIOD
            };
            if self.sweep_mod_clock_counter >= sweep_mod_clock_period {
                self.sweep_mod_clock_counter = 0;

                self.voice5.sweep_mod_clock(&self.mod_table);
            }

            self.noise_clock_counter += 1;
            if self.noise_clock_counter >= NOISE_CLOCK_PERIOD {
                self.noise_clock_counter = 0;

                self.voice6.noise_clock();
            }

            self.sample_clock_counter += 1;
            if self.sample_clock_counter >= SAMPLE_CLOCK_PERIOD {
                self.sample_clock_counter = 0;

                self.sample_clock(audio_frame_sink);
            }
        }
    }

    fn sample_clock(&mut self, audio_frame_sink: &mut Sink<AudioFrame>) {
        let mut acc_left = 0;
        let mut acc_right = 0;

        fn mix_sample<V: Voice>(acc_left: &mut u32, acc_right: &mut u32, voice: &V, voice_output: u32) {
            let (left, right) = if voice.reg_play_control().enable {
                let envelope_level = voice.envelope().level();

                let left_level = if voice.reg_volume().left == 0 || envelope_level == 0 {
                    0
                } else {
                    ((voice.reg_volume().left * envelope_level) >> 3) + 1
                };
                let right_level = if voice.reg_volume().right == 0 || envelope_level == 0 {
                    0
                } else {
                    ((voice.reg_volume().right * envelope_level) >> 3) + 1
                };

                let output_left = (voice_output * left_level) >> 1;
                let output_right = (voice_output * right_level) >> 1;

                (output_left, output_right)
            } else {
                (0, 0)
            };

            *acc_left += left;
            *acc_right += right;
        }

        mix_sample(&mut acc_left, &mut acc_right, &self.voice1, self.voice1.output(&self.wave_tables));
        mix_sample(&mut acc_left, &mut acc_right, &self.voice2, self.voice2.output(&self.wave_tables));
        mix_sample(&mut acc_left, &mut acc_right, &self.voice3, self.voice3.output(&self.wave_tables));
        mix_sample(&mut acc_left, &mut acc_right, &self.voice4, self.voice4.output(&self.wave_tables));
        mix_sample(&mut acc_left, &mut acc_right, &self.voice5, self.voice5.output(&self.wave_tables));
        mix_sample(&mut acc_left, &mut acc_right, &self.voice6, self.voice6.output());

        let output_left = ((acc_left & 0xfff8) << 2) as i16;
        let output_right = ((acc_right & 0xfff8) << 2) as i16;

        audio_frame_sink.append((output_left, output_right));
    }

    fn are_channels_active(&self) -> bool {
        self.voice1.reg_play_control.enable ||
        self.voice2.reg_play_control.enable ||
        self.voice3.reg_play_control.enable ||
        self.voice4.reg_play_control.enable ||
        self.voice5.reg_play_control.enable ||
        self.voice6.reg_play_control.enable
    }
}
