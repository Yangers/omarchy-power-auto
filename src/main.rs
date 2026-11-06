use anyhow::Result;
use clap::Parser;
use futures_util::StreamExt;
use std::path::Path;
use std::time::Duration;
use zbus::Connection;

const BATTERY_THRESHOLD: u32 = 25;
const TEMP_EMERGENCY_HIGH: f64 = 92.0;   // °C -> power-saver immediately
const TEMP_THROTTLE_HIGH: f64 = 83.0;    // °C -> balanced
const TEMP_RECOVERY_LOW: f64 = 74.0;     // °C -> recovered
const FAST_CHARGE_MIN_WATTS: f64 = 10.0; // Watts into battery
const RECOVERY_CYCLES_NEEDED: u32 = 3;   // ~18s (3 * 6s) sustained cool
const THERMAL_POLL_SECS: u64 = 6;

#[zbus::proxy(
    interface = "net.hadess.PowerProfiles",
    default_service = "net.hadess.PowerProfiles",
    default_path = "/net/hadess/PowerProfiles"
)]
trait PowerProfiles {
    #[zbus(property)]
    fn active_profile(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn set_active_profile(&self, profile: &str) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait LoginManager {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[derive(Parser, Debug)]
#[command(
    name = "omarchy-power-auto",
    about = "Automate Omarchy power profiles based on AC, battery state, and thermals."
)]
struct Args {
    /// Run in daemon/monitoring mode.
    #[arg(long)]
    daemon: bool,

    /// Print current power status and recommendation.
    #[arg(long)]
    status: bool,
}

#[derive(Debug, Clone)]
struct PowerStatus {
    on_battery: bool,
    capacity: u32,
    is_fast_charging: bool,
    charge_watts: f64,
}

#[derive(Debug)]
struct TargetDecision {
    target: &'static str,
    reason: String,
    glyph: &'static str,
    urgency: &'static str,
    event_notify: bool,
}

fn get_cpu_temp() -> f64 {
    let mut temps = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let hwmon_path = entry.path();
            if let Ok(files) = std::fs::read_dir(&hwmon_path) {
                for file in files.flatten() {
                    let name = file.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("temp") && name_str.ends_with("_input") {
                        if let Ok(content) = std::fs::read_to_string(file.path()) {
                            if let Ok(val) = content.trim().parse::<f64>() {
                                let t = val / 1000.0;
                                if (10.0..=125.0).contains(&t) {
                                    temps.push(t);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    temps
        .into_iter()
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(55.0)
}

async fn get_power_status(upower: Option<&UPowerProxy<'_>>) -> PowerStatus {
    let mut on_battery = None;
    if let Some(up) = upower {
        if let Ok(val) = up.on_battery().await {
            on_battery = Some(val);
        }
    }

    let on_battery = on_battery.unwrap_or_else(|| {
        let mut ac_online = false;
        if let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with('A') || name_str.starts_with("ucsi") {
                    let online_path = entry.path().join("online");
                    if let Ok(val) = std::fs::read_to_string(online_path) {
                        if val.trim() == "1" {
                            ac_online = true;
                            break;
                        }
                    }
                }
            }
        }
        !ac_online
    });

    let mut capacity = 100;
    let mut bat_status = String::from("Unknown");
    let mut charge_watts = 0.0;

    let bat_dir = Path::new("/sys/class/power_supply/BAT0");
    if bat_dir.exists() {
        if let Ok(val) = std::fs::read_to_string(bat_dir.join("capacity")) {
            if let Ok(c) = val.trim().parse::<u32>() {
                capacity = c;
            }
        }
        if let Ok(val) = std::fs::read_to_string(bat_dir.join("status")) {
            bat_status = val.trim().to_string();
        }
        if let Ok(val) = std::fs::read_to_string(bat_dir.join("power_now")) {
            if let Ok(p) = val.trim().parse::<f64>() {
                charge_watts = p / 1e6;
            }
        } else if let (Ok(v_str), Ok(c_str)) = (
            std::fs::read_to_string(bat_dir.join("voltage_now")),
            std::fs::read_to_string(bat_dir.join("current_now")),
        ) {
            if let (Ok(v), Ok(c)) = (v_str.trim().parse::<f64>(), c_str.trim().parse::<f64>()) {
                charge_watts = (v / 1e6) * (c / 1e6);
            }
        }
    }

    let is_fast_charging =
        !on_battery && bat_status == "Charging" && charge_watts >= FAST_CHARGE_MIN_WATTS;

    PowerStatus {
        on_battery,
        capacity,
        is_fast_charging,
        charge_watts,
    }
}

fn determine_target(
    on_battery: bool,
    capacity: u32,
    is_fast_charging: bool,
    charge_watts: f64,
    cpu_temp: f64,
    in_cooldown: bool,
) -> TargetDecision {
    // 1. On Battery
    if on_battery {
        if capacity <= BATTERY_THRESHOLD {
            return TargetDecision {
                target: "power-saver",
                reason: format!("Low battery ({}%): Switched to Power Saver", capacity),
                glyph: "󰌪",
                urgency: "critical",
                event_notify: true,
            };
        } else if cpu_temp >= 75.0 || in_cooldown {
            return TargetDecision {
                target: "power-saver",
                reason: format!(
                    "Battery Thermal Guard ({:.0}°C): Switched to Power Saver",
                    cpu_temp
                ),
                glyph: "󰌪",
                urgency: "low",
                event_notify: false,
            };
        } else {
            return TargetDecision {
                target: "power-saver",
                reason: format!("On battery ({}%): Power Saver active", capacity),
                glyph: "󰌪",
                urgency: "low",
                event_notify: false,
            };
        }
    }

    // 2. On AC Power
    // 2.1 Critical Overheat Guard (> 92°C or active cooldown)
    if cpu_temp >= TEMP_EMERGENCY_HIGH || in_cooldown {
        let reason_msg = if cpu_temp >= TEMP_EMERGENCY_HIGH {
            format!(
                "Overheat Protection ({:.0}°C): Cooldown to Power Saver",
                cpu_temp
            )
        } else {
            format!(
                "Thermal Cooldown Active ({:.0}°C): Holding Power Saver",
                cpu_temp
            )
        };
        return TargetDecision {
            target: "power-saver",
            reason: reason_msg,
            glyph: "󰌪",
            urgency: "low",
            event_notify: false,
        };
    }

    // 2.2 Fast Charging Protection (Charger inverting > 10W into battery + CPU heat)
    if is_fast_charging {
        return TargetDecision {
            target: "balanced",
            reason: format!(
                "Fast Charging ({:.1}W): Balanced to prevent overheat",
                charge_watts
            ),
            glyph: "󱐋",
            urgency: "normal",
            event_notify: false,
        };
    }

    // 2.3 Thermal Throttle Guard (> 83°C)
    if cpu_temp >= TEMP_THROTTLE_HIGH {
        return TargetDecision {
            target: "balanced",
            reason: format!("Thermal Guard ({:.0}°C): Limiting to Balanced", cpu_temp),
            glyph: "󱐋",
            urgency: "normal",
            event_notify: false,
        };
    }

    // 2.4 AC Connected, Cool, Battery Charged / Idle
    TargetDecision {
        target: "balanced",
        reason: "Plugged in: Balanced mode".to_string(),
        glyph: "󱐋",
        urgency: "low",
        event_notify: false,
    }
}

async fn set_profile(
    pp: &PowerProfilesProxy<'_>,
    target_profile: &str,
    reason: &str,
    glyph: &str,
    urgency: &str,
    send_notify: bool,
) -> bool {
    let current = pp
        .active_profile()
        .await
        .unwrap_or_else(|_| "balanced".to_string());
    if current == target_profile {
        return false;
    }

    if let Err(e) = pp.set_active_profile(target_profile).await {
        eprintln!("Error setting profile {}: {}", target_profile, e);
        return false;
    }

    println!("Profile: {} -> {} ({})", current, target_profile, reason);

    if send_notify {
        let title = "Power Profile";
        let _ = std::process::Command::new("omarchy-notification-send")
            .args(["-g", glyph, "-u", urgency, title, reason])
            .spawn()
            .or_else(|_| {
                std::process::Command::new("notify-send")
                    .args([title, reason])
                    .spawn()
            });
    }

    true
}

async fn apply_once(pp: &PowerProfilesProxy<'_>, up: &UPowerProxy<'_>, force_notify: bool) {
    let status = get_power_status(Some(up)).await;
    let cpu_temp = get_cpu_temp();
    let decision = determine_target(
        status.on_battery,
        status.capacity,
        status.is_fast_charging,
        status.charge_watts,
        cpu_temp,
        false,
    );
    let should_notify = force_notify || decision.event_notify;
    set_profile(
        pp,
        decision.target,
        &decision.reason,
        decision.glyph,
        decision.urgency,
        should_notify,
    )
    .await;
}

struct DaemonState {
    last_on_battery: bool,
    last_under_threshold: bool,
    in_cooldown: bool,
    cooldown_recovery_count: u32,
}

impl DaemonState {
    async fn evaluate_and_apply(
        &mut self,
        pp: &PowerProfilesProxy<'_>,
        up: &UPowerProxy<'_>,
        is_startup: bool,
    ) {
        let status = get_power_status(Some(up)).await;
        let cpu_temp = get_cpu_temp();
        let curr_under_threshold = status.capacity <= BATTERY_THRESHOLD;

        // Thermal hysteresis management:
        // Drop to power-saver when >= 92°C, hold until sustained < 74°C for 3 cycles (~18s)
        if cpu_temp >= TEMP_EMERGENCY_HIGH {
            self.in_cooldown = true;
            self.cooldown_recovery_count = 0;
        } else if self.in_cooldown {
            if cpu_temp < TEMP_RECOVERY_LOW {
                self.cooldown_recovery_count += 1;
                if self.cooldown_recovery_count >= RECOVERY_CYCLES_NEEDED {
                    self.in_cooldown = false;
                    self.cooldown_recovery_count = 0;
                }
            } else {
                self.cooldown_recovery_count = 0;
            }
        }

        // Transitions
        let ac_transition = status.on_battery != self.last_on_battery;
        let threshold_down_transition =
            status.on_battery && curr_under_threshold && !self.last_under_threshold;

        let decision = determine_target(
            status.on_battery,
            status.capacity,
            status.is_fast_charging,
            status.charge_watts,
            cpu_temp,
            self.in_cooldown,
        );

        let mut should_notify = false;
        let mut reason = decision.reason;

        if !is_startup {
            if ac_transition {
                should_notify = true;
                reason = if !status.on_battery {
                    "Plugged into AC".to_string()
                } else {
                    "Switched to Battery".to_string()
                };
            } else if threshold_down_transition {
                should_notify = true;
            } else if decision.event_notify && decision.urgency == "critical" {
                should_notify = true;
            }
        }

        set_profile(
            pp,
            decision.target,
            &reason,
            decision.glyph,
            decision.urgency,
            should_notify,
        )
        .await;

        self.last_on_battery = status.on_battery;
        self.last_under_threshold = curr_under_threshold;
    }
}

async fn run_daemon(conn: &Connection) -> Result<()> {
    println!(
        "omarchy-power-auto daemon started (Battery saver threshold: {}%)...",
        BATTERY_THRESHOLD
    );

    let pp = PowerProfilesProxy::new(conn).await?;
    let up = UPowerProxy::new(conn).await?;
    let login = LoginManagerProxy::new(conn).await?;

    let init_status = get_power_status(Some(&up)).await;
    let mut state = DaemonState {
        last_on_battery: init_status.on_battery,
        last_under_threshold: init_status.capacity <= BATTERY_THRESHOLD,
        in_cooldown: false,
        cooldown_recovery_count: 0,
    };

    // Initial startup: apply without desktop notification spam
    state.evaluate_and_apply(&pp, &up, true).await;

    // D-Bus property change stream on UPower.OnBattery
    let mut on_battery_stream = up.receive_on_battery_changed().await;

    // D-Bus signal stream on login1.Manager.PrepareForSleep
    let mut sleep_stream = login.receive_prepare_for_sleep().await?;

    // 6-second timer interval for thermal monitoring and hysteresis
    let mut ticker = tokio::time::interval(Duration::from_secs(THERMAL_POLL_SECS));
    // First tick fires immediately; consume it since we just evaluated
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = on_battery_stream.next() => {
                // AC state change detected via D-Bus property change!
                state.evaluate_and_apply(&pp, &up, false).await;
            }
            Some(signal) = sleep_stream.next() => {
                if let Ok(args) = signal.args() {
                    // start == false means system just woke up from sleep
                    if !args.start {
                        // Wait 2.0s for USB bus and power supply state to settle
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        state.evaluate_and_apply(&pp, &up, false).await;
                    }
                }
            }
            _ = ticker.tick() => {
                // Periodic thermal and hysteresis poll
                state.evaluate_and_apply(&pp, &up, false).await;
            }
            _ = tokio::signal::ctrl_c() => {
                println!("omarchy-power-auto daemon exiting...");
                break;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let conn = Connection::system().await?;

    if args.status {
        let pp = PowerProfilesProxy::new(&conn).await?;
        let up = UPowerProxy::new(&conn).await?;
        let status = get_power_status(Some(&up)).await;
        let cpu_temp = get_cpu_temp();
        let current = pp
            .active_profile()
            .await
            .unwrap_or_else(|_| "balanced".to_string());
        let decision = determine_target(
            status.on_battery,
            status.capacity,
            status.is_fast_charging,
            status.charge_watts,
            cpu_temp,
            false,
        );

        println!("==================================================");
        println!("  Omarchy Power & Thermal Auto Status");
        println!("==================================================");
        println!(
            "Power Source       : {}",
            if status.on_battery {
                "Battery"
            } else {
                "AC (Mains / USB-C PD)"
            }
        );
        println!("Battery Capacity   : {}%", status.capacity);
        println!(
            "Battery Charging   : {:.2} W (Fast Charging: {})",
            status.charge_watts,
            if status.is_fast_charging {
                "True"
            } else {
                "False"
            }
        );
        println!("CPU Max Temp       : {:.1} °C", cpu_temp);
        println!("Active Profile     : {}", current.to_uppercase());
        println!("Target Profile     : {}", decision.target.to_uppercase());
        println!("Reason             : {}", decision.reason);
        println!("==================================================");
    } else if args.daemon {
        run_daemon(&conn).await?;
    } else {
        let pp = PowerProfilesProxy::new(&conn).await?;
        let up = UPowerProxy::new(&conn).await?;
        apply_once(&pp, &up, false).await;
    }

    Ok(())
}
