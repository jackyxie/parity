// Copyright 2015-2017 Parity Technologies (UK) Ltd.
// This file is part of Parity.

// Parity is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity.  If not, see <http://www.gnu.org/licenses/>.

//! Parity-specific PUB-SUB rpc implementation.

use std::sync::Arc;
use std::time::Duration;
use util::RwLock;

use futures::{self, BoxFuture, Future, Stream, Sink};
use jsonrpc_core::{self as core, Error, MetaIoHandler};
use jsonrpc_pubsub::SubscriptionId;
use jsonrpc_macros::pubsub::Subscriber;
use tokio_timer;

use v1::helpers::GenericPollManager;
use v1::metadata::Metadata;
use v1::traits::PubSub;
use parity_reactor::Remote;

/// Parity PubSub implementation.
pub struct PubSubClient<S: core::Middleware<Metadata>> {
	poll_manager: Arc<RwLock<GenericPollManager<S>>>,
	remote: Remote,
}

impl<S: core::Middleware<Metadata>> PubSubClient<S> {
	/// Creates new `PubSubClient`.
	pub fn new(rpc: MetaIoHandler<Metadata, S>, remote: Remote) -> Self {
		let poll_manager = Arc::new(RwLock::new(GenericPollManager::new(rpc)));
		let pm2 = poll_manager.clone();

		let timer = tokio_timer::wheel()
			.max_capacity(1)
			.initial_capacity(1)
			.tick_duration(Duration::from_millis(100))
			.build();
		let interval = timer.interval(Duration::from_millis(1000));
		// Start ticking
		remote.spawn(interval
			.map_err(|e| warn!("Polling timer error: {:?}", e))
			.for_each(move |_| pm2.read().tick())
		);

		PubSubClient {
			poll_manager: poll_manager,
			remote: remote,
		}
	}
}

impl<S: core::Middleware<Metadata>> PubSub for PubSubClient<S> {
	type Metadata = Metadata;

	fn parity_subscribe(&self, mut meta: Metadata, subscriber: Subscriber<core::Output>, method: String, params: core::Params) {
		// Make sure to get rid of PubSub session otherwise it will never be dropped.
		meta.session = None;

		let mut poll_manager = self.poll_manager.write();
		let (id, receiver) = poll_manager.subscribe(meta, method, params);
		match subscriber.assign_id(SubscriptionId::Number(id as u64)) {
			Ok(sink) => {
				self.remote.spawn(receiver.forward(sink.sink_map_err(|e| {
					warn!("Cannot send notification: {:?}", e);
				})).map(|_| ()))
			},
			Err(_) => {
				poll_manager.unsubscribe(id);
			},
		}
	}

	fn parity_unsubscribe(&self, id: SubscriptionId) -> BoxFuture<bool, Error> {
		let res = if let SubscriptionId::Number(id) = id {
			self.poll_manager.write().unsubscribe(id as usize)
		} else {
			false
		};

		futures::future::ok(res).boxed()
	}
}
