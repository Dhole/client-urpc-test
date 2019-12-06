#![allow(dead_code)]
#![allow(unused_variables)]

extern crate clap;
extern crate serial;

use std::convert;
use std::io::{self, Read, Write};
use std::process;
use std::time::Duration;

use clap::{App, Arg, ArgMatches, SubCommand};
use serial::prelude::*;

#[macro_use]
extern crate urpc;

use urpc::{client, client::OptBufNo, client::OptBufYes, consts};

client_requests! {
    client_requests;
    (1, RequestSendBytes((), OptBufYes, (), OptBufNo)),
    (2, RequestAdd((u8, u8), OptBufNo, u8, OptBufNo)),
    (0, RequestPing([u8; 4], OptBufNo, [u8; 4], OptBufNo))
}

fn main() {
    let mut app = App::new("client")
        .version("0.1")
        .about("uRPC client test")
        .author("Dhole")
        .arg(
            Arg::with_name("baud")
                .help("Set the baud rate")
                .short("b")
                .long("baud")
                .value_name("RATE")
                .default_value("9600")
                .takes_value(true)
                .required(false)
                .validator(|baud| match baud.parse::<usize>() {
                    Ok(_) => Ok(()),
                    Err(e) => Err(format!("{}", e)),
                }),
        )
        .arg(
            Arg::with_name("serial")
                .help("Set the serial device")
                .short("s")
                .long("serial")
                .value_name("DEVICE")
                .default_value("/dev/ttyACM0")
                .takes_value(true)
                .required(false),
        )
        .subcommands(vec![
            SubCommand::with_name("ping").arg(Arg::with_name("arg").index(1)),
            SubCommand::with_name("send_bytes").arg(Arg::with_name("arg").index(1)),
            SubCommand::with_name("add")
                .arg(Arg::with_name("arg1").index(1))
                .arg(Arg::with_name("arg2").index(2)),
        ]);
    let matches = app.clone().get_matches();
    if matches.subcommand_name() == None {
        app.print_help().unwrap();
        return;
    }

    run_subcommand(matches).unwrap_or_else(|e| {
        println!("Error during operation: {:?}", e);
        process::exit(1);
    });
}

fn run_subcommand(matches: ArgMatches) -> Result<(), io::Error> {
    let serial = matches.value_of("serial").unwrap();
    let baud = matches.value_of("baud").unwrap().parse::<usize>().unwrap();

    let mut port_raw = match serial::open(serial) {
        Ok(port) => port,
        Err(e) => {
            println!("Error opening {}: {}", serial, e);
            process::exit(1);
        }
    };
    port_raw
        .configure(&serial::PortSettings {
            baud_rate: serial::BaudRate::from_speed(baud),
            char_size: serial::Bits8,
            parity: serial::ParityNone,
            stop_bits: serial::Stop1,
            flow_control: serial::FlowNone,
        })
        .unwrap_or_else(|e| {
            println!("Error configuring {}: {}", serial, e);
            process::exit(1);
        });
    port_raw
        .set_timeout(Duration::from_secs(16))
        .unwrap_or_else(|e| {
            println!("Error setting timeout for {}: {}", serial, e);
            process::exit(1);
        });

    let mut rpc_client = client::RpcClient::new();
    let mut send_buf = vec![0; 32];

    match matches.subcommand() {
        ("ping", Some(m)) => {
            let arg = m.value_of("arg").unwrap();
            let mut payload = [0; 4];
            &payload.copy_from_slice(arg[..4].as_bytes());
            let mut req = RequestPing::new(payload);
            let n = req
                .request(&mut rpc_client, vec![0; 32], &mut send_buf)
                .unwrap();
            port_raw.write_all(&send_buf[..n])?;

            match ReplyWaiter::new(req, rpc_client).block_rcv_reply(&mut port_raw, 32) {
                Err(e) => println!("Err: {:?}", e),
                Ok(r) => println!("Reply: {:?}", r),
            }
        }
        ("send_bytes", Some(m)) => {
            let arg = m.value_of("arg").unwrap();

            let mut req = RequestSendBytes::new(());
            let n = req
                .request(&arg.as_bytes(), &mut rpc_client, vec![0; 32], &mut send_buf)
                .unwrap();
            dbg!(&send_buf[..n]);
            port_raw.write_all(&send_buf[..n])?;

            match ReplyWaiter::new(req, rpc_client).block_rcv_reply(&mut port_raw, 32) {
                Err(e) => println!("Err: {:?}", e),
                Ok(r) => println!("Reply: {:?}", r),
            }
        }
        ("add", Some(m)) => {
            let arg1 = m.value_of("arg1").unwrap().parse::<u8>().unwrap();
            let arg2 = m.value_of("arg2").unwrap().parse::<u8>().unwrap();
            let mut req = RequestAdd::new((arg1, arg2));
            let n = req
                .request(&mut rpc_client, vec![0; 32], &mut send_buf)
                .unwrap();
            port_raw.write_all(&send_buf[..n])?;

            match ReplyWaiter::new(req, rpc_client).block_rcv_reply(&mut port_raw, 32) {
                Err(e) => println!("Err: {:?}", e),
                Ok(r) => println!("Reply: {:?}", r),
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[derive(Debug)]
enum Error {
    Io(io::Error),
    Urpc(client::Error),
}

impl convert::From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl convert::From<client::Error> for Error {
    fn from(error: client::Error) -> Self {
        Self::Urpc(error)
    }
}

struct ReplyWaiter<R: client::Request, QB: client::OptBuf, PB: client::OptBuf> {
    rpc_client: client::RpcClient,
    req: client::RequestType<R, QB, PB>,
}

impl<R: client::Request, QB: client::OptBuf, PB: client::OptBuf> ReplyWaiter<R, QB, PB> {
    fn new(req: client::RequestType<R, QB, PB>, rpc_client: client::RpcClient) -> Self {
        Self { rpc_client, req }
    }
}

impl<R: client::Request, QB: client::OptBuf> ReplyWaiter<R, QB, client::OptBufYes> {
    fn block_rcv_reply<RD: Read>(
        &mut self,
        r: &mut RD,
        buf_len: usize,
    ) -> Result<(R::P, Vec<u8>), Error> {
        let mut pos = 0;
        let mut read_len = consts::REP_HEADER_LEN;
        let mut recv_buf = vec![0; buf_len];
        loop {
            let mut buf = &mut recv_buf[pos..pos + read_len];
            r.read_exact(&mut buf)?;
            pos += read_len;
            read_len = self.rpc_client.parse(&buf).unwrap().0;
            match self.req.take_reply(&mut self.rpc_client) {
                Some(r) => {
                    return r.map_err(|e| e.into());
                }
                None => {}
            }
        }
    }
}

impl<R: client::Request, QB: client::OptBuf> ReplyWaiter<R, QB, client::OptBufNo> {
    fn block_rcv_reply<RD: Read>(&mut self, r: &mut RD, buf_len: usize) -> Result<R::P, Error> {
        let mut pos = 0;
        let mut read_len = consts::REP_HEADER_LEN;
        let mut recv_buf = vec![0; buf_len];
        loop {
            let mut buf = &mut recv_buf[pos..pos + read_len];
            r.read_exact(&mut buf)?;
            pos += read_len;
            read_len = self.rpc_client.parse(&buf).unwrap().0;
            match self.req.take_reply(&mut self.rpc_client) {
                Some(r) => {
                    return r.map_err(|e| e.into());
                }
                None => {}
            }
        }
    }
}
