//
// Copyright (c) 2017, 2020 ADLINK Technology Inc.
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ADLINK zenoh team, <zenoh@adlink-labs.tech>
//
use async_std::task::sleep;
use clap::{App, Arg};
use futures::prelude::*;
use futures::select;
use std::time::Duration;
use zenoh::config::Config;
use zenoh::prelude::*;
use zenoh::queryable::EVAL;

#[async_std::main]
async fn main() {
    // initiate logging
    env_logger::init();

    let (config, key_expr, value) = parse_args();

    println!("Openning session...");
    let session = zenoh::open(config).await.unwrap();

    println!("Creating Queryable on '{}'...", key_expr);
    let mut queryable = session.queryable(&key_expr).kind(EVAL).await.unwrap();

    println!("Enter 'q' to quit...");
    let mut stdin = async_std::io::stdin();
    let mut input = [0_u8];
    loop {
        select!(
            query = queryable.receiver().next() => {
                let query = query.unwrap();
                println!(">> [Queryable ] Received Query '{}'", query.selector());
                query.reply(Sample::new(key_expr.clone(), value.clone()));
            },

            _ = stdin.read_exact(&mut input).fuse() => {
                match input[0] {
                    b'q' => break,
                    0 => sleep(Duration::from_secs(1)).await,
                    _ => (),
                }
            }
        );
    }
}

fn parse_args() -> (Config, String, String) {
    let args = App::new("zenoh eval example")
        .arg(
            Arg::from_usage("-m, --mode=[MODE] 'The zenoh session mode (peer by default).")
                .possible_values(&["peer", "client"]),
        )
        .arg(Arg::from_usage(
            "-e, --peer=[LOCATOR]...  'Peer locators used to initiate the zenoh session.'",
        ))
        .arg(Arg::from_usage(
            "-l, --listener=[LOCATOR]...   'Locators to listen on.'",
        ))
        .arg(
            Arg::from_usage(
                "-k, --key=[KEYEXPR]        'The key expression matching queries to evaluate.'",
            )
            .default_value("/demo/example/zenoh-rs-eval"),
        )
        .arg(
            Arg::from_usage("-v, --value=[VALUE]      'The value to reply to queries.'")
                .default_value("Eval from Rust!"),
        )
        .arg(Arg::from_usage(
            "-c, --config=[FILE]      'A configuration file.'",
        ))
        .get_matches();

    let mut config = if let Some(conf_file) = args.value_of("config") {
        Config::from_file(conf_file).unwrap()
    } else {
        Config::default()
    };
    if let Some(Ok(mode)) = args.value_of("mode").map(|mode| mode.parse()) {
        config.set_mode(Some(mode)).unwrap();
    }
    match args.value_of("mode").map(|m| m.parse()) {
        Some(Ok(mode)) => {
            config.set_mode(Some(mode)).unwrap();
        }
        Some(Err(())) => panic!("Invalid mode"),
        None => {}
    };
    if let Some(values) = args.values_of("peer") {
        config.peers.extend(values.map(|v| v.parse().unwrap()))
    }
    if let Some(values) = args.values_of("listeners") {
        config.listeners.extend(values.map(|v| v.parse().unwrap()))
    }
    if args.is_present("no-multicast-scouting") {
        config.scouting.multicast.set_enabled(Some(false)).unwrap();
    }

    let key_expr = args.value_of("key").unwrap().to_string();
    let value = args.value_of("value").unwrap().to_string();

    (config, key_expr, value)
}