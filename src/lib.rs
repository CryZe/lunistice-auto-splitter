#![no_std]
#![cfg_attr(
    feature = "nightly",
    feature(type_alias_impl_trait, const_async_blocks)
)]

use core::pin::pin;

use arrayvec::ArrayString;
use asr::{
    future::{next_tick, retry},
    game_engine::unity::il2cpp::{Class, Image, Module, Version},
    print_message,
    time::Duration,
    timer::{self, TimerState},
    watcher::Watcher,
    Address, Address64, Process,
};
use asr_derive::Il2cppClass;
use bytemuck_derive::{Pod, Zeroable};
use futures_util::future::{self, Either};

asr::panic_handler!();

struct GameInfo {
    timer_instance: Address,
    game_manager_instance: Address,
    timer_class: TimerBinding,
    game_manager_class: GameManagerBinding,
}

impl GameInfo {
    async fn load(process: &Process) -> Self {
        let module = Module::wait_attach(process, Version::V2020).await;

        print_message("Found Mono");

        let image = module.wait_get_default_image(process).await;

        print_message("Found Assembly-CSharp");

        let game_manager_class = GameManagerBinding::bind(process, &module, &image).await;
        let game_manager_instance = game_manager_class
            .class()
            .wait_get_static_instance(process, &module, "<Instance>k__BackingField")
            .await;

        print_message(if game_manager_class.is_dlc() {
            "Found GameManager (DLC)"
        } else {
            "Found GameManager (No DLC)"
        });

        let timer_class = Timer::bind(process, &module, &image).await;
        let timer_instance = timer_class
            .class()
            .wait_get_static_instance(
                process,
                &module,
                if game_manager_class.is_dlc() {
                    "<Instance>k__BackingField"
                } else {
                    "_instance"
                },
            )
            .await;

        print_message("Found Timer");

        Self {
            timer_instance,
            game_manager_instance,
            timer_class,
            game_manager_class,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Digits {
    minutes: f32,
    seconds: f32,
    hundredths: f32,
}

impl Digits {
    fn format_into<const N: usize>(&self, string: &mut ArrayString<N>) {
        let mut buffer = itoa::Buffer::new();
        let _ = string.try_push_str(buffer.format(self.minutes as u32));
        let _ = string.try_push(':');
        let seconds = buffer.format(self.seconds as u8);
        if seconds.len() < 2 {
            let _ = string.try_push('0');
        }
        let _ = string.try_push_str(seconds);
        let _ = string.try_push('.');
        let hundredths = buffer.format(self.hundredths as u8);
        if hundredths.len() < 2 {
            let _ = string.try_push('0');
        }
        let _ = string.try_push_str(hundredths);
    }
}

#[derive(Copy, Clone)]
struct GameManager {
    game_state: i32,
    points: i32,
    deaths: i32,
    level_or_scene: LevelOrScene,
}

#[derive(Copy, Clone)]
enum LevelOrScene {
    Level(i32),
    Scene(ArrayString<16>),
}

impl LevelOrScene {
    const LEVEL_1_1: i32 = 0;
    const LEVEL_2_1: i32 = 2;
    const LEVEL_7_2: i32 = 13;
    const LEVEL_7_X: i32 = 14;

    fn is_in_first_level(&self) -> bool {
        match self {
            LevelOrScene::Level(v) => *v == Self::LEVEL_1_1,
            LevelOrScene::Scene(s) => s == "Shrine01",
        }
    }

    fn stage(level: i32) -> i32 {
        ((level / 2) + 1).min(7)
    }

    fn act(level: i32) -> char {
        if level == Self::LEVEL_7_X {
            'X'
        } else if level & 1 == 0 {
            '1'
        } else {
            '2'
        }
    }

    fn format_level_into<const N: usize>(level: i32, string: &mut ArrayString<N>) {
        let mut buffer = itoa::Buffer::new();
        let _ = string.try_push_str(buffer.format(Self::stage(level)));
        let _ = string.try_push('-');
        let _ = string.try_push(Self::act(level));
    }

    fn set_variable<const N: usize>(&self, string: &mut ArrayString<N>) {
        match self {
            LevelOrScene::Level(level) => {
                string.clear();
                Self::format_level_into(*level, string);
                timer::set_variable("Level", string);
            }
            LevelOrScene::Scene(scene) => {
                timer::set_variable("Scene", scene);
            }
        }
    }

    fn is_in_final_level(&self) -> bool {
        match self {
            LevelOrScene::Level(level) => *level >= Self::LEVEL_7_2,
            LevelOrScene::Scene(_) => false,
        }
    }

    fn is_in_credits(&self) -> bool {
        match self {
            LevelOrScene::Level(level) => *level == Self::LEVEL_2_1,
            LevelOrScene::Scene(_) => false,
        }
    }
}

enum GameManagerBinding {
    Original(original::GameManagerBinding),
    Dlc(dlc::GameManagerBinding),
}

impl GameManagerBinding {
    async fn bind(process: &Process, module: &Module, image: &Image) -> Self {
        let original = pin!(original::GameManager::bind(process, module, image));
        let dlc = pin!(dlc::GameManager::bind(process, module, image));
        match future::select(original, dlc).await {
            Either::Left((original, _)) => Self::Original(original),
            Either::Right((dlc, _)) => Self::Dlc(dlc),
        }
    }

    fn class(&self) -> &Class {
        match self {
            GameManagerBinding::Original(original) => original.class(),
            GameManagerBinding::Dlc(dlc) => dlc.class(),
        }
    }

    #[must_use]
    fn is_dlc(&self) -> bool {
        matches!(self, Self::Dlc(..))
    }

    fn read(&self, process: &Process, game_manager_instance: Address) -> Result<GameManager, ()> {
        Ok(match self {
            GameManagerBinding::Original(original) => {
                let game_manager = original.read(process, game_manager_instance)?;
                GameManager {
                    game_state: game_manager.game_state,
                    points: game_manager.points,
                    deaths: game_manager.deaths,
                    level_or_scene: LevelOrScene::Level(game_manager.level),
                }
            }
            GameManagerBinding::Dlc(dlc) => {
                let game_manager = dlc.read(process, game_manager_instance)?;
                GameManager {
                    game_state: game_manager.game_state,
                    points: game_manager.points,
                    deaths: game_manager.deaths,
                    level_or_scene: LevelOrScene::Scene(
                        read_string(process, game_manager.current_scene_ptr).unwrap_or_default(),
                    ),
                }
            }
        })
    }
}

mod original {
    use asr_derive::Il2cppClass;

    #[derive(Copy, Clone, Il2cppClass)]
    pub struct GameManager {
        #[rename = "gameState"]
        pub game_state: i32,
        #[rename = "_points"]
        pub points: i32,
        #[rename = "_deaths"]
        pub deaths: i32,
        #[rename = "currentLevel"]
        pub level: i32,
    }
}

mod dlc {
    use asr::Address64;
    use asr_derive::Il2cppClass;

    #[derive(Copy, Clone, Il2cppClass)]
    pub struct GameManager {
        #[rename = "<GameState>k__BackingField"]
        pub game_state: i32,
        #[rename = "_points"]
        pub points: i32,
        #[rename = "_deaths"]
        pub deaths: i32,
        #[rename = "_currentScene"]
        pub current_scene_ptr: Address64,
    }
}

#[derive(Copy, Clone, Il2cppClass)]
struct Timer {
    #[rename = "currentLevelTime"]
    level_time: f32,
    #[rename = "currentLevelTimeVector"]
    level_time_vector: Digits,
    #[rename = "timerStopped"]
    timer_stopped: bool,
    character: u32,
}

#[allow(unused)]
mod game_state {
    pub const MISSION: i32 = 0;
    pub const TITLE_SCREEN: i32 = 1;
    pub const MENU: i32 = 2;
    pub const CUTSCENE: i32 = 3;
    pub const DEATH: i32 = 4;
    pub const RESPAWN: i32 = 5;
    pub const RESULTS: i32 = 6;
    pub const LOAD: i32 = 7;
}

impl Timer {
    fn character(&self) -> &'static str {
        match self.character {
            0 => "Hana",
            1 => "Toree",
            2 => "Toukie",
            3 => "Accel",
            _ => "Unknown",
        }
    }
}

fn read_string(process: &Process, ptr: Address64) -> Option<ArrayString<16>> {
    let len = process.read::<u32>(ptr + 0x10).ok()? as usize;
    let utf16_buf = &mut [0u16; 16][..len.min(16)];
    let mut utf8_buf = ArrayString::<16>::new();
    process.read_into_slice(ptr + 0x14, utf16_buf).ok()?;
    for c in char::decode_utf16(utf16_buf.iter().copied()) {
        let _ = utf8_buf.try_push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    Some(utf8_buf)
}

#[cfg(not(feature = "nightly"))]
asr::async_main!(stable);
#[cfg(feature = "nightly")]
asr::async_main!(nightly);

async fn main() {
    let mut run_time = Duration::ZERO;
    let mut beyond_first_level = false;

    loop {
        asr::set_tick_rate(1.0);

        let process = retry(|| {
            Process::attach("Lunistice.exe").or_else(|| Process::attach("Lunistice-Demo.exe"))
        })
        .await;

        process
            .until_closes(async {
                let game_info = GameInfo::load(&process).await;

                let mut timer = Watcher::new();
                let mut game_manager = Watcher::new();
                let mut timer_state = Watcher::new();

                asr::set_tick_rate(120.0);

                loop {
                    let game_manager = game_manager.update(
                        game_info
                            .game_manager_class
                            .read(&process, game_info.game_manager_instance)
                            .ok(),
                    );

                    let timer = timer.update(
                        game_info
                            .timer_class
                            .read(&process, game_info.timer_instance)
                            .ok(),
                    );

                    if let (Some(game_manager), Some(timer)) = (game_manager, timer) {
                        let mut buffer = itoa::Buffer::new();
                        timer::set_variable("Points", buffer.format(game_manager.points));
                        timer::set_variable("Resets", buffer.format(game_manager.deaths));

                        let mut string_buffer = ArrayString::<32>::new();
                        timer.level_time_vector.format_into(&mut string_buffer);
                        timer::set_variable("Level Time", &string_buffer);
                        game_manager.level_or_scene.set_variable(&mut string_buffer);
                        timer::set_variable("Character", timer.character());

                        let timer_state = timer_state.update_infallible(timer::state());

                        // We do this here because the runner might start the
                        // timer themselves.
                        if timer_state.changed_from(&TimerState::NotRunning) {
                            run_time = Duration::ZERO;
                            beyond_first_level = false;
                            timer::pause_game_time();
                            timer::set_game_time(run_time);
                        }

                        match timer_state.current {
                            TimerState::NotRunning => {
                                if timer.check(|t| !t.timer_stopped)
                                    && game_manager.level_or_scene.is_in_first_level()
                                {
                                    timer::start();
                                }
                            }
                            TimerState::Paused | TimerState::Running => {
                                if timer.current.level_time < timer.old.level_time {
                                    if !beyond_first_level {
                                        timer::reset();
                                        return;
                                    }
                                    run_time +=
                                        Duration::saturating_seconds_f32(timer.old.level_time);
                                }

                                timer::set_game_time(
                                    run_time + Duration::saturating_seconds_f32(timer.level_time),
                                );

                                if game_manager.check(|g| g.game_state == game_state::RESULTS)
                                    || (game_manager.old.level_or_scene.is_in_final_level()
                                        && game_manager.current.level_or_scene.is_in_credits())
                                {
                                    beyond_first_level = true;
                                    timer::split();
                                }
                            }
                            _ => {}
                        }
                    }

                    next_tick().await;
                }
            })
            .await;
    }
}
