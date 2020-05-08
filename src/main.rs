#![allow(dead_code)]
#![allow(unused_variables)]
mod bc;
mod bc_protocol;
mod config;
mod cmdline;
mod gst;

use backoff::{Error::*, ExponentialBackoff, Operation};
use bc_protocol::BcCamera;
use config::{Config, CameraConfig};
use cmdline::Opt;
use err_derive::Error;
use gst::RtspServer;
use log::error;
use std::fs;
use std::time::Duration;
use std::io::Write;
use structopt::StructOpt;

#[derive(Debug, Error)]
pub enum Error {
    #[error(display="Configuration parsing error")]
    ConfigError(#[error(source)] toml::de::Error),
    #[error(display="Communication error")]
    ProtocolError(#[error(source)] bc_protocol::Error),
    #[error(display="I/O error")]
    IoError(#[error(source)] std::io::Error),
}

fn main() -> Result<(), Error> {
    env_logger::init();
    let opt = Opt::from_args();
    let config: Config = toml::from_str(&fs::read_to_string(opt.config)?)?;

    let rtsp = &RtspServer::new();

    crossbeam::scope(|s| {
        for camera in config.cameras {
            s.spawn(move |_| {
                // TODO handle these errors
                let mut output = rtsp.add_stream(&camera.name).unwrap(); // TODO
                camera_loop(&camera, &mut output)
            });
        }

        rtsp.run(&config.bind_addr);
    }).unwrap();

    Ok(())
}

fn camera_loop(camera_config: &CameraConfig, output: &mut dyn Write) -> Result<(), Error> {
    let mut backoff = ExponentialBackoff {
        initial_interval: Duration::from_secs(5),
        max_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let mut op = || {
        camera_main(camera_config, output)
    };
    op.retry(&mut backoff).map_err(|backoff_err| -> Error {
        match backoff_err {
            Transient(e) => {
                error!("Error streaming from camera {}, will retry: {}", camera_config.name, e);
                e.into()
            }
            Permanent(e) => {
                error!("Permanent error sreaming from camera {}: {}", camera_config.name, e);
                e.into()
            }
        }
    })
}

fn camera_main(camera_config: &CameraConfig, output: &mut dyn Write) -> Result<(), backoff::Error<bc_protocol::Error>> {
    let mut camera = BcCamera::new_with_addr(camera_config.camera_addr)?;

    println!("{}: Connecting to camera at {}", camera_config.name, camera_config.camera_addr);
    camera.connect()?;

    // Authentication failures are permanent; we retry everything else
    camera.login(&camera_config.username, camera_config.password.as_deref()).map_err(|e| {
        match e {
            bc_protocol::Error::AuthFailed => Permanent(e),
            _ => Transient(e)
        }
    })?;

    println!("{}: Connected to camera, starting video stream", camera_config.name);
    camera.start_video(output)?;

    unreachable!()
}
