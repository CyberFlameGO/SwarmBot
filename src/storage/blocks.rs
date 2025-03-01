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

use std::collections::{BinaryHeap, HashMap};
use std::convert::TryFrom;

use float_ord::FloatOrd;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

use crate::client::bot::{ProcessError, WrongArgCount};
use crate::client::pathfind::MinHeapNode;
use crate::schematic::Schematic;
use crate::storage::block::{BlockApprox, BlockKind, BlockLocation, BlockState, SimpleType};
use crate::storage::chunk::{ChunkColumn, ChunkData, HighMemoryChunkSection};
use crate::types::Location;

pub mod cache;

#[derive(Copy, Clone, Hash, Eq, PartialEq)]
pub struct ChunkLocation(pub i32, pub i32);

impl TryFrom<&[&str]> for ChunkLocation {
    type Error = ProcessError;

    fn try_from(value: &[&str]) -> Result<Self, Self::Error> {
        if let [a, b] = value {
            let x = a.parse()?;
            let z = b.parse()?;
            Ok(ChunkLocation(x, z))
        } else {
            Err(WrongArgCount::new(2).into())
        }
    }
}

impl From<BlockLocation> for ChunkLocation {
    fn from(loc: BlockLocation) -> Self {
        Self(loc.x >> 4, loc.z >> 4)
    }
}

impl From<Location> for ChunkLocation {
    fn from(loc: Location) -> Self {
        let block_loc = BlockLocation::from(loc);
        Self::from(block_loc)
    }
}

#[derive(Default)]
pub struct WorldBlocks {
    storage: HashMap<ChunkLocation, ChunkColumn>,
}

struct HeapIter<T> {
    heap: BinaryHeap<T>,
}

impl<T: Ord> Iterator for HeapIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.heap.pop()
    }
}


fn block_chunk_iter(loc: &'a ChunkLocation, column: &'a ChunkData<HighMemoryChunkSection>, selector: impl FnMut(BlockState) -> bool + 'a) -> impl Iterator<Item=BlockLocation> + 'a {
    let start_x = loc.0 << 4;
    let start_z = loc.1 << 4;
    column.select_up(selector).map(move |idx| {
        let x = idx % 16;
        let leftover = idx >> 4;
        let z = leftover % 16;
        let y = leftover / 16;
        BlockLocation::new(x as i32 + start_x, y as i16, z as i32 + start_z)
    })
}

impl WorldBlocks {
    /// A world that is flat at y=0 in a 100 block radius from 0,0
    pub fn flat() -> WorldBlocks {
        let mut world = WorldBlocks::default();
        for x in -100..=100 {
            for z in -100..=100 {
                let loc = BlockLocation::new(x, 0, z);
                world.set_block(loc, BlockState::STONE);
            }
        }
        world
    }

    pub fn first_below(&self, location: BlockLocation) -> Option<(BlockLocation, BlockState)> {
        (0..location.y).rev()
            .map(|y| BlockLocation::new(location.x, y, location.z))
            .find_map(|loc| {
                let exact = self.get_block_exact(loc)?;
                if exact.simple_type() != SimpleType::Solid {
                    None
                } else {
                    Some((loc, exact))
                }
            })
    }

    /// similar to a bedrock floor. (0,0) is guaranteed to be a solid as well as (950, 950)
    pub fn set_random_floor(&mut self) {
        const RADIUS: i32 = 1000;
        let mut rdm = StdRng::seed_from_u64(12338971);
        for x in -RADIUS..=RADIUS {
            for z in -RADIUS..=RADIUS {
                let res = rdm.gen_range(0..5);
                let loc = BlockLocation::new(x, 0, z);
                if res == 0 {
                    self.set_block(loc, BlockState::STONE);
                }
            }
        }

        self.set_block(BlockLocation::default(), BlockState::STONE);
        self.set_block(BlockLocation::new(950, 0, 950), BlockState::STONE);
    }

    pub fn y_slice(&self, origin: BlockLocation, radius: u8, mut selector: impl FnMut(BlockState) -> bool) -> Option<Vec<BlockLocation>> {
        let BlockLocation { x, y, z } = origin;

        let radius = radius as i32;

        let start_x = (x - radius) >> 4;
        let end_x = (x + radius) >> 4;

        let start_z = (z - radius) >> 4;
        let end_z = (z + radius) >> 4;

        let mut res = Vec::new();

        for cx in start_x..=end_x {
            for cz in start_z..=end_z {
                let chunk_loc = ChunkLocation(cx, cz);
                let column = self.get_real_column(chunk_loc)?;
                let states = column.all_at(y as u8);

                let chunk_start_x = cx << 4;
                let chunk_start_z = cz << 4;

                let iter = IntoIterator::into_iter(states)
                    .enumerate()
                    .map(|(idx, state)| {
                        let dx = (idx % 16) as i32;
                        let dz = (idx / 16) as i32;
                        (BlockLocation::new(chunk_start_x + dx, y as i16, chunk_start_z + dz), state)
                    })
                    .filter(|(loc, _)| {
                        let dx = (x - loc.x).abs();
                        let dz = (z - loc.z).abs();
                        let r = dx.max(dz);
                        r <= radius
                    })
                    .filter(|(_, state)| selector(*state))
                    .map(|(loc, _)| loc);

                res.extend(iter);
            }
        }

        Some(res)
    }

    pub fn paste(&mut self, schematic: &Schematic) {
        for (location, state) in schematic.blocks() {
            self.set_block(location, state)
        }
    }

    pub fn add_column(&mut self, location: ChunkLocation, column: ChunkColumn) {
        self.storage.insert(location, column);
    }

    pub fn modify_column(&mut self, location: ChunkLocation, column: ChunkColumn) {
        self.storage.get_mut(&location).unwrap().modify(column);
    }

    pub fn get_block(&self, location: BlockLocation) -> Option<BlockApprox> {
        let BlockLocation { x, y, z } = location;

        let chunk_x = x >> 4;
        let chunk_z = z >> 4;

        let x = (x - (chunk_x << 4)) as u8;
        let z = (z - (chunk_z << 4)) as u8;

        let chunk_x = chunk_x as i32;
        let chunk_z = chunk_z as i32;

        let loc = ChunkLocation(chunk_x, chunk_z);
        let column = self.storage.get(&loc)?;

        // this *should* be either the void or the sky (at least pre-1.17)
        // we do this check here because we want to return None if there is no chunk column in that position
        if !(0..256).contains(&y) {
            return Some(BlockApprox::Realized(BlockState::AIR));
        }

        let block = column.get_block(x, y as u8, z);
        Some(block)
    }

    pub fn closest_in_chunk(&'a self, origin: BlockLocation, selector: impl FnMut(BlockState) -> bool + 'a + Copy) -> Option<BlockLocation> {
        let loc = ChunkLocation::from(origin);
        let chunk = self.storage.get(&loc)?;

        if let ChunkColumn::HighMemory { data } = chunk {
            block_chunk_iter(&loc, data, selector).min_by_key(|&location| FloatOrd(origin.dist2(location)))
        } else {
            None
        }
    }

    pub fn closest(&'a self, origin: BlockLocation, max_chunks: usize, selector: impl FnMut(BlockState) -> bool + 'a + Copy) -> Option<BlockLocation> {
        self.select(origin, max_chunks, selector)
            .min_by_key(|loc| FloatOrd(loc.dist2(origin)))
    }

    pub fn closest_iter(&'a self, origin: BlockLocation, selector: impl FnMut(BlockState) -> bool + 'a + Copy) -> impl Iterator<Item=BlockLocation> + 'a {
        // we use a heap to reduce complexity in case we do not need to use all values
        let heap = self.select(origin, usize::MAX, selector)
            .map(|loc| MinHeapNode::new(loc, loc.dist2(origin)))
            .collect();

        let iterator = HeapIter { heap };
        iterator.map(|node| node.contents)
    }

    fn real_chunks(&self) -> impl Iterator<Item=(&ChunkLocation, &ChunkData<HighMemoryChunkSection>)> + '_ {
        self.storage.iter()
            .filter_map(|(loc, column)| {
                match column {
                    ChunkColumn::HighMemory { data } => {
                        Some((loc, data))
                    }
                    _ => { None }
                }
            })
    }

    pub fn select(&'a self, _around: BlockLocation, max_chunks: usize, selector: impl FnMut(BlockState) -> bool + 'a + Copy) -> impl Iterator<Item=BlockLocation> + 'a {
        self.real_chunks()
            .take(max_chunks)
            .flat_map(move |(loc, column)| {
                block_chunk_iter(loc, column, selector)
            })
    }

    pub fn get_real_column(&self, location: ChunkLocation) -> Option<&ChunkData<HighMemoryChunkSection>> {
        let res = self.storage.get(&location)?;
        match res {
            ChunkColumn::HighMemory { data } => Some(data),
            _ => None
        }
    }

    pub fn get_real_column_mut(&mut self, location: ChunkLocation) -> Option<&mut ChunkData<HighMemoryChunkSection>> {
        let res = self.storage.get_mut(&location)?;
        match res {
            ChunkColumn::HighMemory { data } => Some(data),
            _ => None
        }
    }

    pub fn set_block(&mut self, location: BlockLocation, block: BlockState) {
        let BlockLocation { x, y, z } = location;

        let y = y as u8;

        let chunk_x = x >> 4;
        let chunk_z = z >> 4;

        let x = (x - (chunk_x << 4)) as u8;
        let z = (z - (chunk_z << 4)) as u8;

        let chunk_x = chunk_x as i32;
        let chunk_z = chunk_z as i32;


        let loc = ChunkLocation(chunk_x, chunk_z);

        let column = self.storage.entry(loc).or_default();
        column.set_block(x, y, z, block);
    }

    pub fn get_block_simple(&self, location: BlockLocation) -> Option<SimpleType> {
        let block = self.get_block(location)?;
        Some(block.s_type())
    }

    pub fn get_block_exact(&self, location: BlockLocation) -> Option<BlockState> {
        let block = self.get_block(location)?;
        match block {
            BlockApprox::Realized(state) => Some(state),
            _ => None
        }
    }

    pub fn get_block_kind(&self, location: BlockLocation) -> Option<BlockKind> {
        let block = self.get_block_exact(location)?;
        Some(block.kind())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use test::{Bencher, black_box};

    use assert_matches::assert_matches as am;
    use rand::Rng;

    use crate::schematic::Schematic;
    use crate::storage::block::{BlockApprox, BlockLocation, BlockState};
    use crate::storage::blocks::WorldBlocks;

    #[test]
    fn test_get_set() {
        let mut world = WorldBlocks::default();

        let loc = BlockLocation::new(0, 0, 0);


        {
            world.set_block(loc, BlockState::STONE);
            let got = world.get_block(loc);
            am!(got , Some(BlockApprox::Realized(BlockState::STONE)));
        }

        {
            let up = loc + BlockLocation::new(0, 1, 0);
            let given = BlockState(123);
            world.set_block(up, given);

            let got_up = world.get_block(up);
            am!(got_up , Some(BlockApprox::Realized(_given)));

            let got = world.get_block(loc);
            am!(got , Some(BlockApprox::Realized(BlockState::STONE)));
        }

        {
            world.set_block(loc, BlockState::AIR);
            let got = world.get_block(loc);
            am!(got , Some(BlockApprox::Realized(BlockState::AIR)));
        }
    }

    #[test]
    fn test_full_circle() {
        let mut world = WorldBlocks::default();

        let schematic = {
            let mut spawn_2b2t = OpenOptions::new()
                .read(true)
                .open("test-data/2b2t.schematic")
                .unwrap();

            Schematic::load(&mut spawn_2b2t)
        };


        world.paste(&schematic);

        for (idx, (loc, state)) in schematic.blocks().enumerate() {
            let actual = world.get_block_exact(loc).unwrap();
            assert_eq!(actual, state, "block at {} was supposed to be {:?} but was actually {:?} @ index {}", loc, state, actual, idx);
        }
    }

    #[bench]
    fn bench_get_block(b: &mut Bencher) {
        let mut world = WorldBlocks::default();

        let schematic = {
            let mut spawn_2b2t = OpenOptions::new()
                .read(true)
                .open("test-data/2b2t.schematic")
                .unwrap();

            Schematic::load(&mut spawn_2b2t)
        };


        world.paste(&schematic);

        let origin = schematic.origin().unwrap();


        let mut rand = rand::thread_rng();

        b.iter(|| {
            // Inner closure, the actual test
            let center_x = origin.x + rand.gen_range(3..(schematic.width - 3)) as i32;
            let center_z = origin.z + rand.gen_range(3..(schematic.length - 3)) as i32;

            for x in -3..=3 {
                for z in -3..=3 {
                    for y in 0..256 {
                        let loc = BlockLocation::new(center_x + x, y, center_z + z);
                        black_box(world.get_block(loc));
                    }
                }
            }
        });
    }
}
