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

use crate::client::follow::{Follower, FollowResult};
use crate::client::pathfind::context::MoveNode;
use crate::client::pathfind::implementations::{PlayerProblem, Problem};
use crate::client::pathfind::implementations::novehicle::{BlockGoalCheck, BlockHeuristic, BlockNearGoalCheck, CenterChunkGoalCheck, ChunkHeuristic, TravelProblem};
use crate::client::pathfind::traits::{GoalCheck, Heuristic};
use crate::client::state::global::GlobalState;
use crate::client::state::local::LocalState;
use crate::client::tasks::TaskTrait;
use crate::client::timing::Increment;
use crate::protocol::InterfaceOut;
use crate::storage::block::BlockLocation;
use crate::storage::blocks::ChunkLocation;

pub type ChunkTravelTask = NavigateProblem<ChunkHeuristic, CenterChunkGoalCheck>;
pub type BlockTravelTask = NavigateProblem<BlockHeuristic, BlockGoalCheck>;
pub type BlockTravelNearTask = NavigateProblem<BlockHeuristic, BlockNearGoalCheck>;

impl ChunkTravelTask {
    pub fn new(goal: ChunkLocation, local: &LocalState) -> Self {
        let start = local.physics.location().into();
        let problem = TravelProblem::navigate_center_chunk(start, goal);
        problem.into()
    }
}

impl BlockTravelTask {
    pub fn new(goal: BlockLocation, local: &LocalState) -> Self {
        let start = local.physics.location().into();
        let problem = TravelProblem::navigate_block(start, goal);
        problem.into()
    }
}

pub struct NavigateProblem<H: Heuristic, G: GoalCheck> {
    calculate: bool,
    problem: Box<PlayerProblem<H, G>>,
    follower: Option<Follower>,
}

impl<H: Heuristic, G: GoalCheck> From<PlayerProblem<H, G>> for NavigateProblem<H, G> {
    fn from(problem: PlayerProblem<H, G>) -> Self {
        Self {
            calculate: true,
            problem: box problem,
            follower: None,
        }
    }
}

impl<H: Heuristic + Send + Sync, G: GoalCheck + Send + Sync> TaskTrait for NavigateProblem<H, G> {
    fn tick(&mut self, _out: &mut impl InterfaceOut, local: &mut LocalState, global: &mut GlobalState) -> bool {
        let follower = match self.follower.as_mut() {
            None => return false,
            Some(inner) => inner
        };

        if follower.should_recalc() {
            println!("recalc");
            self.problem.recalc(MoveNode::simple(local.physics.location().into()));
            self.calculate = true;
        }

        match follower.follow(local, global) {
            FollowResult::Failed => {
                println!("failed");
                self.follower = None;
                self.problem.recalc(MoveNode::simple(local.physics.location().into()));
                self.calculate = true;
                false
            }
            FollowResult::InProgress => false,
            FollowResult::Finished => {
                println!("finished!");
                true
            }
        }
    }

    fn expensive(&mut self, end_at: Instant, local: &mut LocalState, global: &GlobalState) {
        if !self.calculate {
            return;
        }

        let res = self.problem.iterate_until(end_at, local, global);
        match res {
            Increment::Finished(res) => {
                self.calculate = false;
                match self.follower.as_mut() {
                    None => self.follower = Follower::new(res),
                    Some(before) => before.merge(res)
                };
            }

            // Nothing as we are still in progress
            Increment::InProgress => {}
        }
    }
}
