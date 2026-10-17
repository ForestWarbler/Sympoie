use std::collections::{HashMap, HashSet};

use sympoie::project::Project;
use sympoie::track::{AudioOutput, Track, TrackId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub index: usize,
    pub id: TrackId,
    pub depth: usize,
    pub has_children: bool,
}

pub fn parents(project: &Project) -> Vec<Option<usize>> {
    let indices: HashMap<TrackId, usize> = project
        .track_ids()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    project
        .tracks()
        .iter()
        .map(|track| match track.mixer().output() {
            AudioOutput::Group(group) => indices.get(&TrackId::Group(group)).copied(),
            _ => None,
        })
        .collect()
}

pub fn children(project: &Project) -> Vec<Vec<usize>> {
    let parents = parents(project);
    let mut children = vec![Vec::new(); parents.len()];
    for (index, parent) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            children[*parent].push(index);
        }
    }
    children
}

pub fn subtree(project: &Project, root: usize) -> Vec<usize> {
    let children = children(project);
    let mut result = Vec::new();
    let mut pending = vec![root];
    while let Some(index) = pending.pop() {
        result.push(index);
        pending.extend(children[index].iter().rev());
    }
    result
}

pub fn is_descendant(parents: &[Option<usize>], index: usize, ancestor: usize) -> bool {
    let mut current = parents[index];
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        current = parents[parent];
    }
    false
}

pub fn display_rows(project: &Project, folded: &HashSet<usize>) -> Vec<Row> {
    let ids: Vec<TrackId> = project.track_ids().collect();
    let parents = parents(project);
    let children = children(project);
    let mut rows = Vec::new();
    let mut pending: Vec<(usize, usize)> = (1..ids.len())
        .rev()
        .filter(|index| parents[*index].is_none())
        .map(|index| (index, 0))
        .collect();
    while let Some((index, depth)) = pending.pop() {
        if matches!(project.tracks()[index], Track::Bus(_)) {
            continue;
        }
        rows.push(Row {
            index,
            id: ids[index],
            depth,
            has_children: !children[index].is_empty(),
        });
        if !folded.contains(&index) {
            pending.extend(
                children[index]
                    .iter()
                    .rev()
                    .map(|child| (*child, depth + 1)),
            );
        }
    }
    rows
}

pub fn mixer_rows(project: &Project, folded: &HashSet<usize>) -> Vec<Row> {
    let mut rows = display_rows(project, folded);
    rows.extend(project.track_ids().enumerate().filter_map(|(index, id)| {
        matches!(id, TrackId::Bus(_)).then_some(Row {
            index,
            id,
            depth: 0,
            has_children: false,
        })
    }));
    rows
}

pub fn track_numbers(project: &Project) -> Vec<usize> {
    let mut numbers = vec![0; project.tracks().len()];
    for (number, row) in mixer_rows(project, &HashSet::new()).iter().enumerate() {
        numbers[row.index] = number;
    }
    if !numbers.is_empty() {
        numbers[0] = numbers.len() - 1;
    }
    numbers
}

pub fn track_label(project: &Project, index: usize) -> String {
    numbered_label(project, index, track_numbers(project)[index])
}

pub fn track_labels(project: &Project) -> Vec<String> {
    track_numbers(project)
        .into_iter()
        .enumerate()
        .map(|(index, number)| numbered_label(project, index, number))
        .collect()
}

fn numbered_label(project: &Project, index: usize, number: usize) -> String {
    if index == 0 {
        return "Master".to_owned();
    }
    let title = track_title(&project.tracks()[index]);
    format!("{number:02} {title}")
}

pub fn track_title(track: &Track) -> String {
    let name = track.name();
    if name.contains('/') || name.contains('\\') {
        std::path::Path::new(name).file_stem().map_or_else(
            || name.to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        )
    } else if name.is_empty() {
        "Untitled".to_owned()
    } else {
        name.to_owned()
    }
}

pub fn kind_label(track: &Track) -> &'static str {
    match track {
        Track::Audio(_) => "AUDIO",
        Track::Midi(_) => "MIDI",
        Track::Group(_) => "GROUP",
        Track::Bus(_) => "BUS",
        Track::Master(_) => "MASTER",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_show_their_members_nested_in_insertion_order() {
        let mut project = Project::new();
        let drums = project.add_audio_track("Drums");
        let band = project.add_group_track("Band");
        let bass = project.add_audio_track("Bass");
        let inner = project.add_group_track("Inner");
        project.add_midi_track("Keys");
        project.set_track_group(drums, Some(inner)).unwrap();
        project.set_track_group(inner, Some(band)).unwrap();
        project.set_track_group(bass, Some(band)).unwrap();

        let rows = display_rows(&project, &HashSet::new());
        let order: Vec<_> = rows.iter().map(|row| (row.index, row.depth)).collect();
        assert_eq!(order, [(2, 0), (3, 1), (4, 1), (1, 2), (5, 0)]);
        assert!(rows[0].has_children);

        let folded = display_rows(&project, &HashSet::from([4]));
        assert_eq!(
            folded.iter().map(|row| row.index).collect::<Vec<_>>(),
            [2, 3, 4, 5]
        );
        assert_eq!(subtree(&project, 2), [2, 3, 4, 1]);
        let parents = parents(&project);
        assert!(is_descendant(&parents, 1, 2));
        assert!(!is_descendant(&parents, 5, 2));
        assert_eq!(track_numbers(&project), [5, 3, 0, 1, 2, 4]);
        assert_eq!(track_label(&project, 2), "00 Band");
        assert_eq!(track_label(&project, 1), "03 Drums");
        assert_eq!(track_label(&project, 0), "Master");
    }
}
