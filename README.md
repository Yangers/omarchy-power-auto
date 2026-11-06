# ⚡ Omarchy Power Auto

> **Intelligent, event-driven Power & Thermal Profile Manager written in Rust for Omarchy Linux (Hyprland + Arch Linux).**  
> Specifically tuned for modern laptops such as the ThinkPad X1 Carbon Gen 9 to optimize battery life, prevent thermal throttling, and eliminate unnecessary fan noise.

---

## ✨ Features

- **Blazing Fast & Zero-Subprocess Architecture**:
  - Replaces frequent Python subprocess forks (`powerprofilesctl get`, `busctl`, `upower --monitor`, `dbus-monitor`) with native, asynchronous D-Bus communication via [`zbus`](https://crates.io/crates/zbus).
  - Stays completely idle in `epoll` until a hardware AC event, system sleep signal, or 6-second thermal timer fires.
  - **Memory footprint**: Reduced from ~35–45 MiB RSS down to **< 2.5 MiB RSS**.
  - **Execution speed**: `--status` query latency dropped from ~380ms down to **< 35ms**.
- **Fast-Charging Protection**:
  - Automatically holds `balanced` mode while USB-C PD is fast-charging (> 10W into the battery) to prevent cumulative chassis heat and thermal throttling.
- **Thermal Hysteresis & Emergency Guard**:
  - **Critical Overheat (≥ 92°C)**: Immediately drops to `power-saver` to cool the SoC.
  - **Cooldown Hysteresis**: Holds `power-saver` until temperatures remain sustainably below `74°C` for 3 consecutive cycles (~18 seconds) before ramping back up.
  - **Throttle Guard (≥ 83°C on AC)**: Restricts profile to `balanced` instead of aggressive boost profiles.
- **Sleep & Wake Safety**:
  - Listens to `org.freedesktop.login1.Manager.PrepareForSleep` over D-Bus.
  - On system resume, allows USB/power supply states to settle before smoothly re-evaluating the profile without desktop notification spam.
- **Event-Driven Desktop Notifications**:
  - Only sends desktop notifications on real physical transitions:
    - Charger connected / disconnected
    - Battery dropping to critical levels (≤ 25%)
    - Critical thermal emergency

---

## 🚀 Installation

```bash
git clone https://github.com/Yangers/omarchy-power-auto.git
cd omarchy-power-auto
chmod +x install.sh
./install.sh
```

---

## 🛠️ CLI Usage

```bash
omarchy-power-auto --status   # Display current battery, wattage, thermals & active profile
omarchy-power-auto --daemon   # Run in persistent background monitoring mode (used by systemd)
omarchy-power-auto            # Run once: evaluate and apply target profile immediately
```

Example status output:
```text
==================================================
  Omarchy Power & Thermal Auto Status
==================================================
Power Source       : Battery
Battery Capacity   : 48%
Battery Charging   : 11.28 W (Fast Charging: False)
CPU Max Temp       : 48.0 °C
Active Profile     : POWER-SAVER
Target Profile     : POWER-SAVER
Reason             : On battery (48%): Power Saver active
==================================================
```

---

## ⚙️ Systemd Service

The installer automatically enables and starts the user service:

```bash
systemctl --user status omarchy-power-auto.service
systemctl --user restart omarchy-power-auto.service
```

---

## 🗑️ Uninstall

```bash
./uninstall.sh
```

---

## 📄 License

MIT License © 2026 Yangers Puentes
