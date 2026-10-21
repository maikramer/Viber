//! Listen server de dois jogadores.
//!
//! `VIBER_NET=host:PORT` abre, `VIBER_NET=join:HOST:PORT` liga. O anfitrião
//! é a autoridade do save e das ações (`attack`, `interact`). O cliente
//! recebe a pose e desenha um fantasma. Sem interest management, sem mundo
//! editável em rede. Desligado quando a variável não existe.
//!
//! A réplica é uma linha de texto por mensagem (`pose x y z yaw` /
//! `act nome`). Não há um segundo simulador: o cliente não corre o mundo.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use bevy::prelude::*;

#[derive(Clone, Debug)]
pub enum NetRole {
    Off,
    Host { port: u16 },
    Join { addr: String },
}

pub fn role_from_env() -> NetRole {
    let Ok(raw) = std::env::var("VIBER_NET") else {
        return NetRole::Off;
    };
    let raw = raw.trim();
    if let Some(port) = raw.strip_prefix("host:") {
        return port
            .parse()
            .map(|port| NetRole::Host { port })
            .unwrap_or(NetRole::Off);
    }
    if let Some(addr) = raw.strip_prefix("join:") {
        return NetRole::Join {
            addr: addr.to_string(),
        };
    }
    NetRole::Off
}

#[derive(Clone, Debug, PartialEq)]
pub enum NetMsg {
    Pose { x: f32, y: f32, z: f32, yaw: f32 },
    Act(String),
}

pub fn encode(msg: &NetMsg) -> String {
    match msg {
        NetMsg::Pose { x, y, z, yaw } => format!("pose {x} {y} {z} {yaw}\n"),
        NetMsg::Act(name) => format!("act {name}\n"),
    }
}

pub fn decode(line: &str) -> Option<NetMsg> {
    let mut parts = line.split_whitespace();
    match parts.next()? {
        "pose" => {
            // `f32::from_str` aceita "nan"/"inf": um par (ou linha corrompida)
            // não pode envenenar o Transform do fantasma.
            let finite = |s: Option<&str>| -> Option<f32> {
                s?.parse::<f32>().ok().filter(|v| v.is_finite())
            };
            let x = finite(parts.next())?;
            let y = finite(parts.next())?;
            let z = finite(parts.next())?;
            let yaw = finite(parts.next())?;
            Some(NetMsg::Pose { x, y, z, yaw })
        }
        "act" => Some(NetMsg::Act(parts.next()?.to_string())),
        _ => None,
    }
}

struct Shared {
    inbound: Mutex<Vec<NetMsg>>,
    outbound: Mutex<Vec<NetMsg>>,
}

#[derive(Resource)]
pub struct NetLink {
    shared: Arc<Shared>,
    pub role: NetRole,
}

#[derive(Component)]
pub struct NetGhost;

fn pump(shared: Arc<Shared>, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut writer = stream.try_clone().ok();
    let mut reader = BufReader::new(stream);
    loop {
        // Escreve ANTES de ler. Com `lines()` + read timeout, o primeiro
        // timeout era um `Err` que matava a thread antes de o primeiro pose
        // sair — as duas pontas ficavam sem rede. Um stall > 50 ms também
        // não pode ser fatal.
        if let Some(writer) = writer.as_mut() {
            if let Ok(mut outbound) = shared.outbound.lock() {
                for msg in outbound.drain(..) {
                    let _ = write!(writer, "{}", encode(&msg));
                }
            }
            let _ = writer.flush();
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(msg) = decode(&line) {
                    if let Ok(mut inbound) = shared.inbound.lock() {
                        inbound.push(msg);
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(_) => break,
        }
    }
}

fn start(role: NetRole) -> Option<NetLink> {
    let shared = Arc::new(Shared {
        inbound: Mutex::new(Vec::new()),
        outbound: Mutex::new(Vec::new()),
    });
    match &role {
        NetRole::Off => return None,
        NetRole::Host { port } => {
            let listener = TcpListener::bind(("0.0.0.0", *port)).ok()?;
            listener.set_nonblocking(true).ok()?;
            let shared_thread = Arc::clone(&shared);
            thread::spawn(move || {
                if let Ok((stream, _)) = listener.accept() {
                    let _ = stream.set_nonblocking(false);
                    pump(shared_thread, stream);
                }
            });
        }
        NetRole::Join { addr } => {
            let stream = TcpStream::connect(addr).ok()?;
            let shared_thread = Arc::clone(&shared);
            thread::spawn(move || pump(shared_thread, stream));
        }
    }
    Some(NetLink { shared, role })
}

pub fn net_sync(
    link: Option<Res<NetLink>>,
    mut commands: Commands,
    time: Res<Time>,
    mut acc: Local<f32>,
    players: Query<&GlobalTransform, With<crate::player::Player>>,
    mut ghosts: Query<&mut Transform, With<NetGhost>>,
    mut actions: Option<ResMut<crate::input_map::ActionState>>,
) {
    let Some(link) = link else { return };
    let (attack_just, interact_just) = actions
        .as_deref()
        .map(|a| (a.attack_just, a.interact_just))
        .unwrap_or((false, false));
    *acc += time.delta_secs();
    if *acc >= 0.05
        && let Some(player) = players.iter().next()
        && let Ok(mut out) = link.shared.outbound.lock()
    {
        *acc = 0.0;
        let p = player.translation();
        let yaw = player.rotation().to_euler(EulerRot::YXZ).0;
        out.push(NetMsg::Pose {
            x: p.x,
            y: p.y,
            z: p.z,
            yaw,
        });
        if attack_just {
            out.push(NetMsg::Act("attack".into()));
        }
        if interact_just {
            out.push(NetMsg::Act("interact".into()));
        }
    }
    let inbound = link
        .shared
        .inbound
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default();
    let is_host = matches!(link.role, NetRole::Host { .. });
    for msg in inbound {
        match msg {
            NetMsg::Pose { x, y, z, yaw } => {
                if let Some(mut ghost) = ghosts.iter_mut().next() {
                    ghost.translation = Vec3::new(x, y, z);
                    ghost.rotation = Quat::from_rotation_y(yaw);
                } else {
                    commands.spawn((
                        NetGhost,
                        Transform::from_xyz(x, y, z).with_rotation(Quat::from_rotation_y(yaw)),
                        Visibility::Inherited,
                        Name::new("net-peer"),
                    ));
                }
            }
            // O anfitrião é a autoridade das ações: sem isto o `act` que o
            // cliente enviava era descodificado e deitado fora.
            NetMsg::Act(name) if is_host => {
                if let Some(actions) = actions.as_deref_mut() {
                    match name.as_str() {
                        "attack" => actions.attack_just = true,
                        "interact" => actions.interact_just = true,
                        _ => {}
                    }
                }
            }
            NetMsg::Act(_) => {}
        }
    }
}

pub struct NetPlayPlugin;

impl Plugin for NetPlayPlugin {
    fn build(&self, app: &mut App) {
        if let Some(link) = start(role_from_env()) {
            app.insert_resource(link);
        }
        app.add_systems(Update, net_sync);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_and_action_roundtrip() {
        let pose = NetMsg::Pose {
            x: 1.0,
            y: 2.0,
            z: -3.0,
            yaw: 0.5,
        };
        assert_eq!(decode(encode(&pose).trim()), Some(pose));
        let act = NetMsg::Act("attack".into());
        assert_eq!(decode(encode(&act).trim()), Some(act));
        assert!(decode("nope").is_none());
    }

    #[test]
    fn missing_env_is_off() {
        // Não mexe no ambiente global se já estiver definido por outro teste.
        if std::env::var_os("VIBER_NET").is_none() {
            assert!(matches!(role_from_env(), NetRole::Off));
        }
    }
}
