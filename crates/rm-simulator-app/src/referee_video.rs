// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Opt-in referee video: the pilot's view as the custom-client video stream.
//!
//! Enabled by the `referee-link` Cargo feature and `--referee-video`. A second
//! camera follows the gameplay camera without the HUD and renders 1280 x 720
//! at 30 frames per second while a match runs. Each frame is read back from
//! the GPU, encoded to HEVC by an `ffmpeg` child process with libx265, and sent
//! over UDP in the official format: one access unit per frame, split into
//! packets of at most 1400 payload bytes behind an 8-byte big-endian header
//! (frame number, fragment index, access unit size).
use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use bevy::camera::{Exposure, RenderTarget};
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{TextureFormat, TextureUsages};

use crate::controls::PlayerCamera;

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const FPS: u32 = 30;
/// Largest UDP payload after the header.
const MAX_PAYLOAD: usize = 1400;
/// Frames waiting for the encoder; newer frames are dropped while it is busy.
const QUEUE: usize = 2;

/// The encoder process and the frames on their way to it.
#[derive(Resource)]
pub struct RefereeVideo {
    encoder: Child,
    frames: SyncSender<Vec<u8>>,
    period: Duration,
    next: Instant,
}

impl Drop for RefereeVideo {
    fn drop(&mut self) {
        let _ = self.encoder.kill();
    }
}

/// The camera that renders the video.
#[derive(Component)]
struct VideoCamera;

/// The entity that reads the video image back from the GPU.
#[derive(Component)]
struct VideoReadback(Handle<Image>);

/// Starts the encoder and the sender for video to `address`.
pub fn start(address: SocketAddr) -> anyhow::Result<RefereeVideo> {
    let socket = UdpSocket::bind(if address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    socket.connect(address)?;
    let size = format!("{WIDTH}x{HEIGHT}");
    let rate = FPS.to_string();
    // Zero latency: no B-frames or lookahead, a keyframe every second, and an
    // access unit delimiter in front of every frame to split the stream on.
    let x265 = format!("keyint={FPS}:aud=1:repeat-headers=1:log-level=error");
    let mut encoder = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo"])
        .args(["-pix_fmt", "rgba", "-s", &size, "-framerate", &rate])
        .args(["-i", "pipe:0", "-an", "-c:v", "libx265"])
        .args(["-preset", "ultrafast", "-tune", "zerolatency", "-b:v", "3M"])
        .args(["-x265-params", &x265, "-pix_fmt", "yuv420p"])
        .args(["-flush_packets", "1", "-f", "hevc", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("cannot start ffmpeg; referee video needs ffmpeg with libx265 on PATH")?;
    let stdin = encoder.stdin.take().expect("stdin is piped");
    let stdout = encoder.stdout.take().expect("stdout is piped");
    let (frames, queued) = sync_channel(QUEUE);
    std::thread::Builder::new()
        .name("referee-video-in".into())
        .spawn(move || feed(queued, stdin))?;
    std::thread::Builder::new()
        .name("referee-video-out".into())
        .spawn(move || send(stdout, &socket))?;
    println!("referee video: HEVC {size} at {FPS} fps to udp://{address}");
    Ok(RefereeVideo {
        encoder,
        frames,
        period: Duration::from_secs(1) / FPS,
        next: Instant::now(),
    })
}

fn feed(frames: Receiver<Vec<u8>>, mut stdin: ChildStdin) {
    for frame in frames {
        if let Err(error) = stdin.write_all(&frame) {
            eprintln!("referee video encoder stopped: {error}");
            return;
        }
    }
}

fn send(mut stdout: impl Read, socket: &UdpSocket) {
    let mut units = AccessUnits::default();
    let mut frame: u16 = 0;
    let mut bytes = vec![0; 64 * 1024];
    loop {
        let read = match stdout.read(&mut bytes) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        for unit in units.push(&bytes[..read]) {
            for packet in fragment(frame, &unit) {
                // Loss is part of the protocol; the client waits for a keyframe.
                let _ = socket.send(&packet);
            }
            frame = frame.wrapping_add(1);
        }
    }
}

/// Splits an Annex B HEVC stream into access units at each access unit
/// delimiter. A unit is complete once the next one begins.
#[derive(Default)]
struct AccessUnits {
    buffer: Vec<u8>,
}

impl AccessUnits {
    fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(bytes);
        let mut units = Vec::new();
        while let Some(first) = delimiter(&self.buffer, 0) {
            // Bytes before the first delimiter belong to no unit.
            self.buffer.drain(..first);
            let Some(next) = delimiter(&self.buffer, 4) else {
                break;
            };
            units.push(self.buffer.drain(..next).collect());
        }
        units
    }
}

/// Offset of the first start code at or after `from` that opens an access
/// unit delimiter NAL (type 35), including a fourth leading zero byte.
fn delimiter(data: &[u8], from: usize) -> Option<usize> {
    let at = data
        .windows(4)
        .skip(from)
        .position(|window| window == [0, 0, 1, 35 << 1])?
        + from;
    Some(if at > from && data[at - 1] == 0 {
        at - 1
    } else {
        at
    })
}

/// UDP packets for one access unit: frame number, fragment index and unit
/// size, big endian, then at most `MAX_PAYLOAD` bytes.
fn fragment(frame: u16, unit: &[u8]) -> Vec<Vec<u8>> {
    let size = u32::try_from(unit.len()).unwrap_or(u32::MAX);
    unit.chunks(MAX_PAYLOAD)
        .enumerate()
        .map(|(index, payload)| {
            let index = u16::try_from(index).unwrap_or(u16::MAX);
            let mut packet = Vec::with_capacity(8 + payload.len());
            packet.extend_from_slice(&frame.to_be_bytes());
            packet.extend_from_slice(&index.to_be_bytes());
            packet.extend_from_slice(&size.to_be_bytes());
            packet.extend_from_slice(payload);
            packet
        })
        .collect()
}

/// Adds the video camera and sends its frames while a match runs.
pub struct RefereeVideoPlugin;

impl Plugin for RefereeVideoPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup.after(crate::scene::setup_camera))
            .add_systems(PostUpdate, pace);
    }
}

fn setup(
    mut commands: Commands,
    appearance: Res<crate::scene::Appearance>,
    player: Single<Entity, With<PlayerCamera>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut image = Image::new_target_texture(WIDTH, HEIGHT, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let image = images.add(image);
    let mut camera = commands.spawn((
        VideoCamera,
        Camera3d::default(),
        Camera {
            is_active: false,
            order: -1,
            ..default()
        },
        RenderTarget::Image(image.clone().into()),
        rm_simulator_render::lighting::skybox(&mut images),
        Projection::Perspective(PerspectiveProjection {
            fov: 70_f32.to_radians(),
            near: 0.01,
            far: 300.0,
            ..default()
        }),
        Transform::IDENTITY,
        ChildOf(*player),
    ));
    rm_simulator_render::camera_appearance(&mut camera, appearance.0);
    commands.spawn(VideoReadback(image)).observe(forward);
}

/// Renders and reads back one frame per period during a match, and keeps the
/// video camera's exposure with the gameplay camera's.
fn pace(
    mut commands: Commands,
    mut video: ResMut<RefereeVideo>,
    screen: Res<State<crate::loading::Screen>>,
    player: Single<Option<&Exposure>, (With<PlayerCamera>, Without<VideoCamera>)>,
    camera: Single<(&mut Camera, Option<&mut Exposure>), With<VideoCamera>>,
    readback: Single<(Entity, &VideoReadback)>,
) {
    let now = Instant::now();
    let due = *screen.get() == crate::loading::Screen::InMatch && now >= video.next;
    if due {
        video.next = (video.next + video.period).max(now);
    }
    let (mut camera, exposure) = camera.into_inner();
    camera.is_active = due;
    if let (Some(mut exposure), Some(player)) = (exposure, *player) {
        *exposure = *player;
    }
    let (entity, VideoReadback(image)) = *readback;
    if due {
        commands
            .entity(entity)
            .insert(Readback::texture(image.clone()));
    } else {
        commands.entity(entity).remove::<Readback>();
    }
}

fn forward(frame: On<ReadbackComplete>, video: Res<RefereeVideo>) {
    if frame.data.len() != (WIDTH * HEIGHT * 4) as usize {
        warn_once!("referee video readback has {} bytes", frame.data.len());
        return;
    }
    match video.frames.try_send(frame.data.clone()) {
        Ok(()) | Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => warn_once!("referee video encoder stopped"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUD: [u8; 7] = [0, 0, 0, 1, 0x46, 0x01, 0x50];

    #[test]
    fn stream_splits_at_each_delimiter_across_reads() {
        let first = [&AUD[..], &[0, 0, 1, 0x40, 0x01, 9, 9]].concat();
        let second = [&AUD[..], &[0, 0, 1, 0x02, 0x01, 7]].concat();
        let stream = [&[0xff, 0xff][..], &first, &second, &AUD].concat();
        let mut units = AccessUnits::default();
        let mut out = Vec::new();
        // One byte at a time, so start codes straddle reads.
        for byte in &stream {
            out.extend(units.push(std::slice::from_ref(byte)));
        }
        assert_eq!(out, [first, second]);
        // The last unit waits for the next delimiter.
        assert_eq!(units.buffer, AUD);
    }

    #[test]
    fn fragments_carry_the_big_endian_header() {
        let unit = vec![5; MAX_PAYLOAD + 10];
        let packets = fragment(0x0102, &unit);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0][..8], [1, 2, 0, 0, 0, 0, 0x05, 0x82]);
        assert_eq!(packets[1][..8], [1, 2, 0, 1, 0, 0, 0x05, 0x82]);
        assert_eq!(packets[0].len(), 8 + MAX_PAYLOAD);
        assert_eq!(packets[1].len(), 8 + 10);
    }
}
