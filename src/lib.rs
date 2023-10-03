#![no_std]
#![cfg_attr(
    feature = "nightly",
    feature(type_alias_impl_trait, const_async_blocks)
)]

use arrayvec::ArrayString;
use asr::{
    future::next_tick,
    game_engine::unity::il2cpp::{Module, Version},
    print_message,
    time::Duration,
    timer::{self, TimerState},
    watcher::Watcher,
    Address, Process,
};
use asr_derive::Il2cppClass;
use bytemuck_derive::{Pod, Zeroable};

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

        let timer_class = Timer::bind(process, &module, &image).await;
        let timer_instance = timer_class
            .class()
            .wait_get_static_instance(process, &module, "_instance")
            .await;

        print_message("Found Timer");

        let game_manager_class = GameManager::bind(process, &module, &image).await;
        let game_manager_instance = game_manager_class
            .class()
            .wait_get_static_instance(process, &module, "<Instance>k__BackingField")
            .await;

        print_message("Found GameManager");

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

impl GameManager {
    fn stage(&self) -> i32 {
        ((self.level / 2) + 1).min(7)
    }

    fn act(&self) -> char {
        if self.level == LEVEL_7_X {
            'X'
        } else if self.level & 1 == 0 {
            '1'
        } else {
            '2'
        }
    }

    fn format_level_into<const N: usize>(&self, string: &mut ArrayString<N>) {
        let mut buffer = itoa::Buffer::new();
        let _ = string.try_push_str(buffer.format(self.stage()));
        let _ = string.try_push('-');
        let _ = string.try_push(self.act());
    }
}

#[derive(Copy, Clone, Il2cppClass)]
struct GameManager {
    #[rename = "gameState"]
    game_state: i32,
    #[rename = "_points"]
    points: i32,
    #[rename = "_deaths"]
    deaths: i32,
    #[rename = "currentLevel"]
    level: i32,
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

const LEVEL_1_1: i32 = 0;
const LEVEL_2_1: i32 = 2;
const LEVEL_7_2: i32 = 13;
const LEVEL_7_X: i32 = 14;

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
            _ => "Unknown",
        }
    }
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

        let process = Process::wait_attach("Lunistice.exe").await;
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
                        string_buffer.clear();
                        game_manager.format_level_into(&mut string_buffer);
                        timer::set_variable("Level", &string_buffer);
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
                                    && game_manager.level == LEVEL_1_1
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
                                    || (game_manager.old.level >= LEVEL_7_2
                                        && game_manager.current.level == LEVEL_2_1)
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
