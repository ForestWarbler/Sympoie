use std::collections::HashSet;

use sympoie::project::Project;
use sympoie::track::{AudioOutput, TrackId};

use super::edit::{EditResult, track_id};
use super::rows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackDrop {
    Before(usize),
    After(usize),
    IntoGroup(usize),
}

struct MovePlan {
    order: Vec<usize>,
    output: AudioOutput,
    changed: bool,
}

fn full_order(project: &Project) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            rows::mixer_rows(project, &HashSet::new())
                .into_iter()
                .map(|row| row.index),
        )
        .collect()
}

fn plan_move(project: &Project, source: usize, target: TrackDrop) -> EditResult<MovePlan> {
    let track = project
        .tracks()
        .get(source)
        .ok_or("Track no longer exists")?;
    if source == 0 {
        return Err("The Master track cannot be moved".into());
    }
    if matches!(track, sympoie::track::Track::Bus(_)) {
        return Err("Bus tracks are managed in the Mixer".into());
    }
    let (TrackDrop::Before(destination)
    | TrackDrop::After(destination)
    | TrackDrop::IntoGroup(destination)) = target;
    if destination == source && !matches!(target, TrackDrop::IntoGroup(_)) {
        return Ok(MovePlan {
            order: (0..project.tracks().len()).collect(),
            output: track.mixer().output(),
            changed: false,
        });
    }
    if destination == 0 || destination >= project.tracks().len() || destination == source {
        return Err("Choose another track as the destination".into());
    }
    if matches!(project.tracks()[destination], sympoie::track::Track::Bus(_)) {
        return Err("Bus tracks are managed in the Mixer".into());
    }
    let parents = rows::parents(project);
    if rows::is_descendant(&parents, destination, source) {
        return Err("A group cannot be moved into its own members".into());
    }
    let output = match target {
        TrackDrop::Before(_) | TrackDrop::After(_) => {
            if parents[source] != parents[destination] {
                return Err("Drag within the current group, or use Exit Group first".into());
            }
            track.mixer().output()
        }
        TrackDrop::IntoGroup(_) => {
            let TrackId::Group(group) = track_id(project, destination)? else {
                return Err("Drop onto a group track to join it".into());
            };
            if let Some(parent) = parents[source]
                && destination != parent
                && !rows::is_descendant(&parents, destination, parent)
            {
                return Err("Use Exit Group before moving into another group".into());
            }
            AudioOutput::Group(group)
        }
    };
    let before = full_order(project);
    let moving = rows::subtree(project, source);
    let mut order: Vec<_> = before
        .iter()
        .copied()
        .filter(|index| !moving.contains(index))
        .collect();
    let anchor = order
        .iter()
        .position(|index| *index == destination)
        .expect("destination remains");
    let insertion = match target {
        TrackDrop::Before(_) => anchor,
        TrackDrop::After(_) | TrackDrop::IntoGroup(_) => {
            anchor
                + 1
                + order[anchor + 1..]
                    .iter()
                    .take_while(|&&index| rows::is_descendant(&parents, index, destination))
                    .count()
        }
    };
    order.splice(insertion..insertion, moving);
    let changed = order != before || output != track.mixer().output();
    Ok(MovePlan {
        order,
        output,
        changed,
    })
}

pub fn can_drop_track(project: &Project, source: usize, target: TrackDrop) -> bool {
    plan_move(project, source, target).is_ok_and(|plan| plan.changed)
}

fn apply_plan(project: &mut Project, source: usize, plan: MovePlan) -> EditResult<Vec<usize>> {
    if !plan.changed {
        return Ok((0..project.tracks().len()).collect());
    }
    if project.tracks()[source].mixer().output() != plan.output {
        super::edit::set_mixer(
            project,
            source,
            project.tracks()[source].mixer().with_output(plan.output),
        )?;
    }
    project
        .reorder_tracks(&plan.order)
        .map_err(|error| error.to_string())
}

pub fn move_track(
    project: &mut Project,
    source: usize,
    target: TrackDrop,
) -> EditResult<Vec<usize>> {
    let plan = plan_move(project, source, target)?;
    apply_plan(project, source, plan)
}

pub fn exit_group(project: &mut Project, source: usize) -> EditResult<Vec<usize>> {
    track_id(project, source)?;
    let parents = rows::parents(project);
    let parent = parents[source].ok_or("This track is not in a group")?;
    let output = project.tracks()[parent].mixer().output();
    let moving = rows::subtree(project, source);
    let mut order = full_order(project);
    order.retain(|index| !moving.contains(index));
    let anchor = order
        .iter()
        .position(|index| *index == parent)
        .expect("parent remains");
    let insertion = anchor
        + 1
        + order[anchor + 1..]
            .iter()
            .take_while(|&&index| rows::is_descendant(&parents, index, parent))
            .count();
    order.splice(insertion..insertion, moving);
    apply_plan(
        project,
        source,
        MovePlan {
            order,
            output,
            changed: true,
        },
    )
}
