#![no_std]
#![allow(non_snake_case)] // TODO: Fix

use arrayvec::ArrayString;
use asr_dotnet::{
    asr::{
        self,
        time::Duration,
        timer::{self, TimerState},
        watcher::Watcher,
        Process,
    },
    MonoAssembly, MonoClass, MonoClassBinding, MonoImage, Ptr,
};
use bytemuck::{Pod, Zeroable};
use spinning_top::{const_spinlock, Spinlock};

#[cfg(all(not(test), target_arch = "wasm32"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

struct GameInfo {
    timer_instance: Ptr,
    game_manager_instance: Ptr,
    timer_binding: TimerBinding,
    game_manager_binding: GameManagerBinding,
}

struct ProcessInfo {
    process: Process,
    game_info: Option<GameInfo>,
}

impl GameInfo {
    fn load(process: &Process) -> Result<Self, ()> {
        let module = process.get_module("GameAssembly.dll").map_err(drop)?;
        // _assembliesTrg signature scan
        let mut arr: Ptr<Ptr<MonoAssembly>> = process.read(module + 0x25D4848u64).map_err(drop)?;

        let image = loop {
            let ptr = arr.read(process)?;
            if ptr.is_null() {
                return Err(());
            }
            let mono_assembly = ptr.read(process)?;
            if mono_assembly
                .aname
                .name
                .read_str(process, |name| name == b"Assembly-CSharp")
            {
                break mono_assembly.image.read(process)?;
            }
            arr = arr.offset(1);
        };

        asr::print_message("Found Assembly-CSharp");

        let timer_binding = Timer::bind(&image, process)?;
        let timer_instance = timer_binding.class.find_singleton(process, "_instance")?;

        asr::print_message("Found Timer");

        let game_manager_binding = GameManager::bind(&image, process)?;
        let game_manager_instance = game_manager_binding
            .class
            .find_singleton(process, "<Instance>k__BackingField")?;

        asr::print_message("Found GameManager");

        Ok(Self {
            timer_instance,
            game_manager_instance,
            timer_binding,
            game_manager_binding,
        })
    }
}

impl ProcessInfo {
    fn new(process: Process) -> Self {
        Self {
            process,
            game_info: None,
        }
    }
}

#[repr(C)]
#[derive(Default, Copy, Clone, Pod, Zeroable, Debug)]
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
        (self.currentLevel / 2) + 1
    }

    fn act(&self) -> i32 {
        (self.currentLevel & 1) + 1
    }

    fn format_level_into<const N: usize>(&self, string: &mut ArrayString<N>) {
        let mut buffer = itoa::Buffer::new();
        let _ = string.try_push_str(buffer.format(self.stage()));
        let _ = string.try_push('-');
        let _ = string.try_push_str(buffer.format(self.act()));
    }
}

#[derive(Copy, Clone, Default, MonoClassBinding)]
struct GameManager {
    gameState: i32,
    _points: i32,
    _deaths: i32,
    currentLevel: i32,
}

#[derive(Copy, Clone, Default, MonoClassBinding)]
struct Timer {
    currentLevelTime: f32,
    currentLevelTimeVector: Digits,
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
            _ => "Unknown",
        }
    }
}

#[derive(Default)]
struct State {
    process_info: Option<ProcessInfo>,
    timer: Watcher<Timer>,
    game_manager: Watcher<GameManager>,
    run_time: f32,
}

impl State {
    fn update(&mut self) {
        if self.process_info.is_none() {
            self.process_info = Process::attach("Lunistice.exe").map(ProcessInfo::new);
        }
        if let Some(process_info) = &mut self.process_info {
            if !process_info.process.is_open() {
                self.process_info = None;
                return;
            }

            if process_info.game_info.is_none() {
                process_info.game_info = GameInfo::load(&process_info.process).ok();
            }

            if let Some(game_info) = &process_info.game_info {
                let game_manager = self.game_manager.update(
                    game_info
                        .game_manager_binding
                        .load(&process_info.process, game_info.game_manager_instance)
                        .ok(),
                );

                let timer = self.timer.update(
                    game_info
                        .timer_binding
                        .load(&process_info.process, game_info.timer_instance)
                        .ok(),
                );

                if let (Some(game_manager), Some(timer)) = (game_manager, timer) {
                    let mut buffer = itoa::Buffer::new();
                    timer::set_variable("Points", buffer.format(game_manager._points));
                    timer::set_variable("Resets", buffer.format(game_manager._deaths));

                    let mut string_buffer = ArrayString::<32>::new();
                    timer.currentLevelTimeVector.format_into(&mut string_buffer);
                    timer::set_variable("Level Time", &string_buffer);
                    string_buffer.clear();
                    game_manager.format_level_into(&mut string_buffer);
                    timer::set_variable("Level", &string_buffer);
                    timer::set_variable("Character", timer.character());

                    match timer::state() {
                        TimerState::NotRunning => {
                            if game_manager.old.currentLevel == 1
                                && game_manager.current.currentLevel == 2
                            {
                                self.run_time = 0.0;
                                timer::start();
                                timer::pause_game_time();
                            }
                        }
                        TimerState::Paused | TimerState::Running => {
                            if timer.current.currentLevelTime < timer.old.currentLevelTime {
                                self.run_time += timer.old.currentLevelTime;
                            }
                            timer::set_game_time(Duration::seconds_f32(
                                self.run_time + timer.currentLevelTime,
                            ));
                            if game_manager.check(|g| g.gameState == game_state::RESULTS) {
                                timer::split();
                            }
                        }
                        TimerState::Ended => {}
                    }
                }
            }
        }
    }
}

static STATE: Spinlock<State> = const_spinlock(State {
    process_info: None,
    timer: Watcher::new(),
    game_manager: Watcher::new(),
    run_time: 0.0,
});

#[no_mangle]
pub extern "C" fn update() {
    STATE.lock().update();
}
