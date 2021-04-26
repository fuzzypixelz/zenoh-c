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
use super::proto::ZenohMessage;
use super::session::defaults::QUEUE_PRIO_DATA;
use super::SessionTransport;
use zenoh_util::zread;

impl SessionTransport {
    #[inline(always)]
    pub(super) fn schedule_first_fit(&self, msg: ZenohMessage) {
        let guard = zread!(self.links);
        for cl in guard.iter() {
            let link = cl.get_link();
            if msg.is_reliable() && link.is_reliable() {
                let c_cl = cl.clone();
                drop(guard);
                c_cl.schedule_zenoh_message(msg, QUEUE_PRIO_DATA);
                return;
            }
            if !msg.is_reliable() && !link.is_reliable() {
                let c_cl = cl.clone();
                drop(guard);
                c_cl.schedule_zenoh_message(msg, QUEUE_PRIO_DATA);
                return;
            }
        }
        match guard.get(0) {
            Some(cl) => {
                let c_cl = cl.clone();
                drop(guard);
                c_cl.schedule_zenoh_message(msg, QUEUE_PRIO_DATA);
            }
            None => log::trace!("Message dropped because the session has no links: {}", msg),
        }
    }
}
