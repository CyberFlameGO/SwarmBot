/*
 * Copyright (c) 2021 Andrew Gazelka - All Rights Reserved.
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use std::time::Instant;

use crate::client::state::global::GlobalState;
use crate::client::state::local::LocalState;
use crate::client::tasks::{Task, TaskTrait};
use crate::protocol::InterfaceOut;

pub struct LazyTask<T: Lazy> {
    inner: Option<Box<Task>>,
    create_task: Option<T>,
}

pub trait Lazy {
    fn create(&self, local: &mut LocalState, global: &GlobalState) -> Task;
}

impl<T: Lazy> From<T> for LazyTask<T> {
    fn from(block: T) -> Self {
        Self {
            inner: None,
            create_task: Some(block),
        }
    }
}

impl<T: Lazy> LazyTask<T> {
    fn get(&mut self, local: &mut LocalState, global: &GlobalState) -> &mut Task {
        if self.inner.is_none() {
            let f = self.create_task.take().unwrap();
            self.inner = Some(Box::new(f.create(local, global)));
        }

        self.inner.as_mut().unwrap()
    }
}

impl<T: Lazy> TaskTrait for LazyTask<T> {
    fn tick(&mut self, out: &mut impl InterfaceOut, local: &mut LocalState, global: &mut GlobalState) -> bool {
        let task = self.get(local, global);
        task.tick(out, local, global)
    }

    fn expensive(&mut self, end_at: Instant, local: &mut LocalState, global: &GlobalState) {
        let task = self.get(local, global);
        task.expensive(end_at, local, global);
    }
}
