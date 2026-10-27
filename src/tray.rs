use std::time::{Duration, Instant};

use sysinfo::{MemoryRefreshKind, System};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    Charging,
    Discharging,
    Full,
}

pub struct Tray {
    sys: System,
    battery: Option<starship_battery::Manager>,
    /// None until the first refresh (subtracting from Instant::now() can panic soon after boot).
    last_sys: Option<Instant>,
    last_battery: Option<Instant>,
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub battery_pct: Option<(f32, Charge)>,
}

impl Default for Tray {
    fn default() -> Self {
        Self::new()
    }
}

impl Tray {
    pub fn new() -> Self {
        let mut t = Self {
            sys: System::new(),
            battery: starship_battery::Manager::new().ok(),
            last_sys: None,
            last_battery: None,
            cpu: 0.0,
            mem_used: 0,
            mem_total: 0,
            battery_pct: None,
        };
        t.refresh();
        t
    }

    /// Refresh stats when they are due. Returns true if anything was updated.
    pub fn refresh(&mut self) -> bool {
        let mut changed = false;
        if self.last_sys.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
            self.sys.refresh_cpu_usage();
            self.sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
            self.cpu = self.sys.global_cpu_usage();
            self.mem_used = self.sys.used_memory();
            self.mem_total = self.sys.total_memory();
            self.last_sys = Some(Instant::now());
            changed = true;
        }
        if self.last_battery.is_none_or(|t| t.elapsed() >= Duration::from_secs(30)) {
            self.last_battery = Some(Instant::now());
            self.battery_pct = self.read_battery();
            changed = true;
        }
        changed
    }

    fn read_battery(&self) -> Option<(f32, Charge)> {
        use starship_battery::State;
        use starship_battery::units::ratio::percent;
        let bat = self.battery.as_ref()?.batteries().ok()?.flatten().next()?;
        let charge = match bat.state() {
            State::Charging => Charge::Charging,
            State::Full => Charge::Full,
            _ => Charge::Discharging,
        };
        Some((bat.state_of_charge().get::<percent>(), charge))
    }
}
