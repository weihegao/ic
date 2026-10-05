//! Multi-host extension of the Local (QEMU) system-test backend.
//!
//! The Local backend boots every VM of a group on the driver's own host. This
//! module lets it spread the VMs over several hosts instead, so a testnet's size
//! is no longer bounded by one machine. It is opt-in through
//! [`HOSTS_ENV`] (`LOCAL_BACKEND_HOSTS`), e.g.
//!
//! ```text
//! LOCAL_BACKEND_HOSTS=local=3,ubuntu@172.22.42.30=3,ubuntu@172.22.42.31=3
//! ```
//!
//! VMs fill the listed hosts in order, each up to its slot count (`=N`, omitted
//! means unbounded). `local` is the driver's host; every other entry is an SSH
//! target that must accept key authentication, allow passwordless `sudo` (for TAP
//! devices and the bridge) and have KVM, `qemu-system-x86_64`, `qemu-img` and the
//! OVMF firmware installed. When the variable is unset the backend behaves
//! exactly as before.
//!
//! All hosts share one layer-2 segment: each host gets a bridge with the group's
//! bridge name, joined by a VXLAN port in a full mesh (head-end replication of
//! broadcast/unknown traffic to every other host). The nodes therefore keep the
//! group's `/64`, the driver's gateway, file-server and log addresses, and the
//! driver's `dnsmasq` unchanged. VXLAN costs 50 bytes per packet, so every guest
//! NIC advertises a smaller MTU ([`overlay_mtu`]) through virtio-net `host_mtu`.
//!
//! A remote VM's disks live on its host: the group's base image is sent there
//! once (sparse) and cached under [`REMOTE_ROOT`], and the VM gets its own qcow2
//! overlay next to its OVMF variable store, pid-file and QMP socket. Its serial
//! console is mirrored into the local `console.log` so `log_consoles_task` still
//! sees it.
//!
//! This only works when the driver runs in the host's network namespace (e.g.
//! via `bazel run` or a `--script_path` runner), not inside a sandboxed
//! `bazel test` action, which has no route to the other hosts.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Environment variable listing the hosts VMs may be placed on.
pub const HOSTS_ENV: &str = "LOCAL_BACKEND_HOSTS";
/// Environment variable overriding the guest MTU used with remote hosts.
pub const OVERLAY_MTU_ENV: &str = "LOCAL_BACKEND_OVERLAY_MTU";
/// Name of the driver's own host in [`HOSTS_ENV`].
pub const LOCAL: &str = "local";
/// Guest MTU: a 1500-byte underlay minus the 50-byte VXLAN encapsulation.
const DEFAULT_OVERLAY_MTU: u32 = 1450;
/// IANA VXLAN port.
const VXLAN_PORT: u16 = 4789;
/// Root of the backend's state on remote hosts (base-image cache, per-group VM dirs).
pub const REMOTE_ROOT: &str = "/var/tmp/ictest";

/// One entry of [`HOSTS_ENV`]: a host and how many VMs it may run.
#[derive(Clone, Debug, PartialEq)]
pub struct HostSlots {
    /// [`LOCAL`] or an SSH target such as `ubuntu@172.22.42.30`.
    pub host: String,
    pub slots: usize,
}

/// Parse [`HOSTS_ENV`]. Returns `None` when it is unset or empty (single-host mode).
pub fn host_plan() -> Result<Option<Vec<HostSlots>>> {
    let Ok(raw) = std::env::var(HOSTS_ENV) else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let mut plan = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (host, slots) = match entry.split_once('=') {
            Some((h, n)) => (
                h.trim(),
                n.trim()
                    .parse::<usize>()
                    .with_context(|| format!("{HOSTS_ENV}: bad slot count in {entry:?}"))?,
            ),
            None => (entry, usize::MAX),
        };
        // Hosts end up in shell scripts and `ip` commands: allow only safe characters.
        if !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._-:".contains(c))
        {
            bail!("{HOSTS_ENV}: unsupported characters in host {host:?}");
        }
        plan.push(HostSlots {
            host: host.to_string(),
            slots,
        });
    }
    Ok((!plan.is_empty()).then_some(plan))
}

/// The distinct remote (non-[`LOCAL`]) hosts of `plan`, in order.
pub fn remote_hosts(plan: &[HostSlots]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in plan {
        if h.host != LOCAL && !out.contains(&h.host) {
            out.push(h.host.clone());
        }
    }
    out
}

/// Whether VMs may be placed on other hosts, i.e. the overlay is in use.
pub fn is_multihost() -> bool {
    matches!(host_plan(), Ok(Some(plan)) if !remote_hosts(&plan).is_empty())
}

/// Guest MTU to advertise when the overlay is in use.
pub fn overlay_mtu() -> u32 {
    std::env::var(OVERLAY_MTU_ENV)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_OVERLAY_MTU)
}

/// The guest MTU to use for this group: `Some` only in multi-host mode.
pub fn guest_mtu() -> Option<u32> {
    is_multihost().then(overlay_mtu)
}

/// The address part of an SSH target (`user@addr` -> `addr`).
pub fn ssh_addr(target: &str) -> &str {
    target.rsplit_once('@').map_or(target, |(_, a)| a)
}

/// Deterministic 24-bit VXLAN network identifier for a group.
pub fn vni(group_name: &str) -> u32 {
    use ic_crypto_sha2::Sha256;
    let h = Sha256::hash(format!("vxlan/{group_name}").as_bytes());
    let v = u32::from_be_bytes([0, h[0], h[1], h[2]]);
    v.max(1)
}

/// VXLAN interface name for a group (`vx-` + 10 hex chars, within `IFNAMSIZ`).
pub fn vxlan_name(group_name: &str) -> String {
    use ic_crypto_sha2::Sha256;
    let h = Sha256::hash(format!("vxlan/{group_name}").as_bytes());
    format!("vx-{}", hex::encode(&h[0..5]))
}

/// Single-quote `s` for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn ssh_command(target: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=15",
        target,
    ]);
    cmd
}

/// Run `script` with `bash -s` on `target`; returns stdout, fails on a non-zero exit.
pub fn ssh_script(target: &str, script: &str) -> Result<String> {
    let mut child = ssh_command(target)
        .arg("bash -s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning ssh to {target}"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(script.as_bytes())
        .with_context(|| format!("sending script to {target}"))?;
    let out = child
        .wait_with_output()
        .with_context(|| format!("waiting for ssh to {target}"))?;
    if !out.status.success() {
        bail!(
            "script on {target} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The driver host's underlay address on the route to `peer`.
pub fn local_underlay_ip(peer: &str) -> Result<String> {
    let out = Command::new("ip")
        .args(["-o", "route", "get", peer])
        .output()
        .context("running `ip route get`")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut it = text.split_whitespace();
    while let Some(tok) = it.next() {
        if tok == "src" {
            if let Some(ip) = it.next() {
                return Ok(ip.to_string());
            }
        }
    }
    bail!("could not determine the local address towards {peer}: {text}")
}

/// Pick (and persist) the host `vm_name` runs on: the first host in `plan` with a
/// free slot. Returns `None` for the driver's own host. The placement is stored
/// under `working_dir` behind a lock, so every process of the group agrees.
pub fn assign_host(
    working_dir: &Path,
    vm_name: &str,
    plan: &[HostSlots],
) -> Result<Option<String>> {
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(working_dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(working_dir.join("placement.lock"))?;
    nix::fcntl::flock(lock.as_raw_fd(), nix::fcntl::FlockArg::LockExclusive)
        .context("locking VM placement")?;
    let path = working_dir.join("placement.json");
    let mut placement: BTreeMap<String, String> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parsing placement.json")?,
        Err(_) => BTreeMap::new(),
    };
    let as_opt = |h: &str| (h != LOCAL).then(|| h.to_string());
    if let Some(h) = placement.get(vm_name) {
        return Ok(as_opt(h));
    }
    for hs in plan {
        let used = placement.values().filter(|v| **v == hs.host).count();
        if used < hs.slots {
            placement.insert(vm_name.to_string(), hs.host.clone());
            std::fs::write(&path, serde_json::to_vec_pretty(&placement)?)?;
            return Ok(as_opt(&hs.host));
        }
    }
    bail!("{HOSTS_ENV}: all slots are taken, cannot place VM {vm_name}")
}

/// Directory of a VM on a remote host.
pub fn remote_vm_dir(bridge: &str, vm: &str) -> String {
    format!("{REMOTE_ROOT}/{bridge}/vms/{vm}")
}

/// QMP socket of a remote VM (short `/tmp` path, see `LocalBackend::qmp_socket_path`).
pub fn remote_qmp_path(remote_vm_dir: &str) -> String {
    use ic_crypto_sha2::Sha256;
    let h = Sha256::hash(remote_vm_dir.as_bytes());
    format!("/tmp/ictest-qmp-{}.sock", hex::encode(&h[0..8]))
}

/// Script that creates (or re-creates) a host's side of the group overlay: the
/// group bridge without addresses, and a VXLAN port flooding to every other host.
fn group_up_script(
    bridge: &str,
    vx: &str,
    vni: u32,
    mtu: u32,
    self_ip: &str,
    peers: &[String],
) -> String {
    let fdb: String = peers
        .iter()
        .filter(|p| p.as_str() != self_ip)
        .map(|p| format!("sudo -n bridge fdb append 00:00:00:00:00:00 dev {vx} dst {p}\n"))
        .collect();
    format!(
        "set -e
command -v qemu-system-x86_64 >/dev/null || {{ echo 'qemu-system-x86_64 not installed' >&2; exit 1; }}
command -v qemu-img >/dev/null || {{ echo 'qemu-img not installed' >&2; exit 1; }}
[ -r /usr/share/OVMF/OVMF_CODE_4M.fd ] || {{ echo 'OVMF firmware (ovmf) not installed' >&2; exit 1; }}
[ -w /dev/kvm ] || {{ echo '/dev/kvm not writable by '$(id -un) >&2; exit 1; }}
mkdir -p {REMOTE_ROOT}/image_cache {REMOTE_ROOT}/{bridge}/vms
sudo -n ip link del {vx} 2>/dev/null || true
sudo -n ip link show {bridge} >/dev/null 2>&1 || sudo -n ip link add name {bridge} type bridge
sudo -n ip link set dev {bridge} mtu {mtu} up
sudo -n ip link add {vx} type vxlan id {vni} dstport {VXLAN_PORT} local {self_ip}
{fdb}sudo -n ip link set dev {vx} mtu {mtu} master {bridge}
sudo -n ip link set dev {vx} up
"
    )
}

/// Bring up a remote host's side of the group overlay.
pub fn remote_group_up(
    target: &str,
    bridge: &str,
    vx: &str,
    vni: u32,
    mtu: u32,
    all_ips: &[String],
) -> Result<()> {
    let script = group_up_script(bridge, vx, vni, mtu, ssh_addr(target), all_ips);
    ssh_script(target, &script).map(|_| ())
}

/// Stop every VM of the group on a remote host and remove its bridge, ports and files.
/// Best-effort: the shared base-image cache is kept for later groups.
pub fn remote_group_down(target: &str, bridge: &str) -> Result<()> {
    let dir = format!("{REMOTE_ROOT}/{bridge}");
    let script = format!(
        "for f in {dir}/vms/*/qemu.pid; do [ -f \"$f\" ] && kill $(cat \"$f\") 2>/dev/null; done
sleep 3
for f in {dir}/vms/*/qemu.pid; do [ -f \"$f\" ] && kill -9 $(cat \"$f\") 2>/dev/null; done
for p in $(ls /sys/class/net/{bridge}/brif 2>/dev/null); do sudo -n ip link del \"$p\" 2>/dev/null; done
sudo -n ip link del {bridge} 2>/dev/null
rm -rf {dir}
true
"
    );
    ssh_script(target, &script).map(|_| ())
}

/// Make sure `target` has the base image `base` (cached by file name, which is
/// content-keyed by `LocalBackend::ensure_base_image`), sending it sparsely if not.
/// Returns its remote path.
pub fn ensure_remote_base(
    target: &str,
    base: &Path,
    working_dir: &Path,
    logger: &slog::Logger,
) -> Result<String> {
    use std::os::unix::io::AsRawFd;
    let name = base
        .file_name()
        .context("base image has no file name")?
        .to_string_lossy()
        .into_owned();
    let remote = format!("{REMOTE_ROOT}/image_cache/{name}");
    // One transfer per (host, image) at a time; other VMs for that host wait here.
    let lock_path = working_dir.join("image_cache").join(format!(
        "{name}.{}.remote.lock",
        ssh_addr(target).replace(':', "_")
    ));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    nix::fcntl::flock(lock.as_raw_fd(), nix::fcntl::FlockArg::LockExclusive)
        .context("locking remote base-image transfer")?;
    if ssh_script(target, &format!("test -f {remote}")).is_ok() {
        return Ok(remote);
    }
    let dir = base.parent().context("base image has no parent dir")?;
    let incoming = format!("{REMOTE_ROOT}/image_cache/.in-{name}");
    slog::info!(
        logger,
        "Sending base image {} to {target}:{remote} (sparse)",
        base.display()
    );
    let start = Instant::now();
    let remote_cmd = format!(
        "rm -rf {incoming} && mkdir -p {incoming} && tar -C {incoming} -xSf - && mv {incoming}/{name} {remote} && chmod 444 {remote} && rmdir {incoming}"
    );
    let pipeline = format!(
        "tar -C {} -cSf - {} | ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 {} {}",
        shell_quote(&dir.to_string_lossy()),
        shell_quote(&name),
        target,
        shell_quote(&remote_cmd)
    );
    let status = Command::new("sh")
        .arg("-c")
        .arg(&pipeline)
        .status()
        .context("running base-image transfer")?;
    if !status.success() {
        bail!(
            "sending base image {} to {target} failed ({status})",
            base.display()
        );
    }
    slog::info!(logger, "Base image on {target} after {:?}", start.elapsed());
    Ok(remote)
}

/// Copy a small local file (a config disk) to `target:remote` unless it is already there.
pub fn upload_if_missing(target: &str, local: &Path, remote: &str) -> Result<()> {
    if ssh_script(target, &format!("test -f {}", shell_quote(remote))).is_ok() {
        return Ok(());
    }
    let file =
        std::fs::File::open(local).with_context(|| format!("opening {}", local.display()))?;
    let parent = remote.rsplit_once('/').map_or(".", |(p, _)| p);
    let out = ssh_command(target)
        .arg(format!(
            "mkdir -p {} && cat > {}.tmp && chmod 600 {}.tmp && mv {}.tmp {}",
            shell_quote(parent),
            shell_quote(remote),
            shell_quote(remote),
            shell_quote(remote),
            shell_quote(remote)
        ))
        .stdin(file)
        .output()
        .with_context(|| format!("uploading {} to {target}", local.display()))?;
    if !out.status.success() {
        bail!(
            "uploading {} to {target}:{remote} failed: {}",
            local.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Parameters of [`remote_start_script`].
pub struct RemoteStart<'a> {
    pub vm_dir: &'a str,
    pub base: &'a str,
    pub overlay_size: Option<&'a str>,
    pub bridge: &'a str,
    pub taps: Vec<&'a str>,
    pub mtu: u32,
    pub ovmf_vars_template: &'a str,
    pub pid: &'a str,
    pub qmp: &'a str,
    pub qemu_args: &'a [String],
}

/// Script that prepares a remote VM (overlay, varstore, TAPs) and starts QEMU,
/// mirroring what `LocalBackend::start_vm` does locally.
pub fn remote_start_script(p: &RemoteStart) -> String {
    let d = p.vm_dir;
    let size = p.overlay_size.map(|s| format!(" {s}")).unwrap_or_default();
    let taps: String = p
        .taps
        .iter()
        .map(|t| {
            format!(
                "sudo -n ip link del {t} 2>/dev/null || true
sudo -n ip tuntap add dev {t} mode tap user $(id -un)
sudo -n ip link set dev {t} mtu {mtu} master {bridge}
sudo -n ip link set dev {t} up
",
                mtu = p.mtu,
                bridge = p.bridge
            )
        })
        .collect();
    let args: Vec<String> = p.qemu_args.iter().map(|a| shell_quote(a)).collect();
    format!(
        "set -e
mkdir -p {d}
[ -f {d}/primary.qcow2 ] || {{ qemu-img create -q -f qcow2 -F raw -b {base} {d}/primary.qcow2{size}; chmod 600 {d}/primary.qcow2; }}
[ -f {d}/OVMF_VARS.fd ] || {{ cp {vars} {d}/OVMF_VARS.fd; chmod 600 {d}/OVMF_VARS.fd; }}
{taps}rm -f {pid} {qmp}
exec qemu-system-x86_64 {args}
",
        base = p.base,
        vars = p.ovmf_vars_template,
        pid = p.pid,
        qmp = p.qmp,
        args = args.join(" ")
    )
}

/// Stop a remote QEMU via its pid-file: SIGTERM, a 5 s grace period, then SIGKILL.
pub fn remote_stop_qemu(target: &str, pid_file: &str) -> Result<()> {
    let script = format!(
        "p=$(cat {pid_file} 2>/dev/null) || exit 0
kill $p 2>/dev/null
for i in $(seq 50); do [ -d /proc/$p ] || break; sleep 0.1; done
if [ -d /proc/$p ] && grep -q '^qemu-' /proc/$p/comm; then kill -9 $p; fi
rm -f {pid_file}
"
    );
    ssh_script(target, &script).map(|_| ())
}

/// Send a no-argument QMP command to a remote VM's monitor socket.
pub fn remote_qmp(target: &str, socket: &str, execute: &str) -> Result<()> {
    let script = format!(
        "python3 - <<'PY'
import socket
s = socket.socket(socket.AF_UNIX)
s.settimeout(10)
s.connect('{socket}')
f = s.makefile('rw')
f.readline()
f.write('{{\"execute\":\"qmp_capabilities\"}}\\n'); f.flush(); f.readline()
f.write('{{\"execute\":\"{execute}\"}}\\n'); f.flush(); f.readline()
PY
"
    );
    ssh_script(target, &script).map(|_| ())
}

/// Poll until the remote QEMU recorded in `pid_file` exits; `true` if it did within `timeout`.
pub fn remote_await_exit(target: &str, pid_file: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let alive = ssh_script(
            target,
            &format!("p=$(cat {pid_file} 2>/dev/null) || exit 1; [ -d /proc/$p ]"),
        )
        .is_ok();
        if !alive {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Mirror a remote VM's serial console into `local` (appending), so the console
/// tailer of the group sees it like a local VM's. The follower is a detached
/// `ssh … tail -F` whose pid is kept in `pid_file`; it resumes after what `local`
/// already holds, so restarting a VM does not duplicate earlier output.
pub fn follow_remote_console(
    target: &str,
    remote: &str,
    local: &Path,
    pid_file: &Path,
) -> Result<()> {
    stop_console_follower(pid_file);
    let have = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
    let cmd = format!(
        "nohup ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=15 {target} {} >> {} 2>/dev/null < /dev/null & echo $!",
        shell_quote(&format!("tail -c +{} -F {remote} 2>/dev/null", have + 1)),
        shell_quote(&local.to_string_lossy())
    );
    let out = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .context("starting console follower")?;
    std::fs::write(pid_file, String::from_utf8_lossy(&out.stdout).trim())
        .with_context(|| format!("writing {}", pid_file.display()))?;
    Ok(())
}

/// Stop a console follower started by [`follow_remote_console`], if any.
pub fn stop_console_follower(pid_file: &Path) {
    if let Ok(pid) = std::fs::read_to_string(pid_file) {
        if let Ok(pid) = pid.trim().parse::<i32>() {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    }
    let _ = std::fs::remove_file(pid_file);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn vxlan_name_fits_ifnamsiz_and_vni_is_24_bit() {
        let n = vxlan_name("some-group--1234");
        assert!(n.len() <= 15, "{n}");
        let v = vni("some-group--1234");
        assert!(v > 0 && v < (1 << 24));
    }

    #[test]
    fn ssh_addr_strips_user() {
        assert_eq!(ssh_addr("ubuntu@172.22.42.30"), "172.22.42.30");
        assert_eq!(ssh_addr("172.22.42.30"), "172.22.42.30");
    }

    #[test]
    fn placement_fills_hosts_in_order_and_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let plan = vec![
            HostSlots {
                host: LOCAL.into(),
                slots: 1,
            },
            HostSlots {
                host: "u@10.0.0.2".into(),
                slots: 2,
            },
        ];
        assert_eq!(assign_host(dir.path(), "a", &plan).unwrap(), None);
        assert_eq!(
            assign_host(dir.path(), "b", &plan).unwrap(),
            Some("u@10.0.0.2".into())
        );
        assert_eq!(
            assign_host(dir.path(), "c", &plan).unwrap(),
            Some("u@10.0.0.2".into())
        );
        assert!(assign_host(dir.path(), "d", &plan).is_err());
        // Re-asking for a placed VM returns the same host.
        assert_eq!(
            assign_host(dir.path(), "b", &plan).unwrap(),
            Some("u@10.0.0.2".into())
        );
    }
}
