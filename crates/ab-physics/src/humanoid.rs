//! Humanoid ragdolls for any character rig.
//!
//! Character rigs name their bones in many ways: `mixamorig:LeftUpLeg`, `thigh_l`,
//! `thigh.L`, `J_Bip_L_UpperLeg`, `Bip01 L Thigh`... [`HumanoidMap::detect`] reads the names
//! (and, failing that, the shape of the skeleton) and finds the ~17 bones a ragdoll needs:
//! hips, spine, chest, neck, head, and each side's upper arm, lower arm, hand, upper leg,
//! lower leg and foot. Any of them can be set by hand when a rig is unusual.
//!
//! [`create_humanoid_ragdoll`] then builds a ragdoll shaped like a person rather than like the
//! rig: boxes for the pelvis, belly and chest, a sphere for the head, capsules for the limbs,
//! boxes for the feet; masses from human proportions; elbows and knees that only bend the
//! right way, cone joints with natural limits for hips, shoulders, spine, neck and ankles.
//! Bones without a body of their own (fingers, toes, twist and clavicle bones, extra spine
//! bones) follow their parents.

use crate::components::{BodyKind, PhysicsBody3D};
use crate::joint::{Joint3D, JointKind};
use crate::nodes::PhysicsNodes;
use crate::ragdoll::{Ragdoll, RagdollBone};
use crate::shape::Shape;
use crate::PhysicsError;
use ab_core::{Affine3A, Mat3, Quat, Vec3};
use ab_scene::{NodeHandle, SceneTree};

/// The bones of a humanoid that a ragdoll is built on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HumanBone {
    Hips,
    Spine,
    Chest,
    Neck,
    Head,
    LeftUpperArm,
    LeftLowerArm,
    LeftHand,
    RightUpperArm,
    RightLowerArm,
    RightHand,
    LeftUpperLeg,
    LeftLowerLeg,
    LeftFoot,
    RightUpperLeg,
    RightLowerLeg,
    RightFoot,
}

impl HumanBone {
    pub const COUNT: usize = 17;

    pub const ALL: [HumanBone; Self::COUNT] = [
        Self::Hips,
        Self::Spine,
        Self::Chest,
        Self::Neck,
        Self::Head,
        Self::LeftUpperArm,
        Self::LeftLowerArm,
        Self::LeftHand,
        Self::RightUpperArm,
        Self::RightLowerArm,
        Self::RightHand,
        Self::LeftUpperLeg,
        Self::LeftLowerLeg,
        Self::LeftFoot,
        Self::RightUpperLeg,
        Self::RightLowerLeg,
        Self::RightFoot,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Name in scene files: `hips`, `left_upper_arm`, ...
    pub fn key(self) -> &'static str {
        match self {
            Self::Hips => "hips",
            Self::Spine => "spine",
            Self::Chest => "chest",
            Self::Neck => "neck",
            Self::Head => "head",
            Self::LeftUpperArm => "left_upper_arm",
            Self::LeftLowerArm => "left_lower_arm",
            Self::LeftHand => "left_hand",
            Self::RightUpperArm => "right_upper_arm",
            Self::RightLowerArm => "right_lower_arm",
            Self::RightHand => "right_hand",
            Self::LeftUpperLeg => "left_upper_leg",
            Self::LeftLowerLeg => "left_lower_leg",
            Self::LeftFoot => "left_foot",
            Self::RightUpperLeg => "right_upper_leg",
            Self::RightLowerLeg => "right_lower_leg",
            Self::RightFoot => "right_foot",
        }
    }

    /// Name for people: `Left upper arm`.
    pub fn label(self) -> String {
        let key = self.key().replace('_', " ");
        let mut chars = key.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default()
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.key() == key)
    }

    /// Bones a ragdoll cannot do without: hips, head and the four limbs down to the
    /// elbows and knees. Spine, chest, neck, hands and feet improve it when found.
    pub fn is_required(self) -> bool {
        matches!(
            self,
            Self::Hips
                | Self::Head
                | Self::LeftUpperArm
                | Self::LeftLowerArm
                | Self::RightUpperArm
                | Self::RightLowerArm
                | Self::LeftUpperLeg
                | Self::LeftLowerLeg
                | Self::RightUpperLeg
                | Self::RightLowerLeg
        )
    }
}

/// The bone node of each [`HumanBone`] of a character.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HumanoidMap {
    bones: [Option<NodeHandle>; HumanBone::COUNT],
}

impl HumanoidMap {
    pub fn get(&self, bone: HumanBone) -> Option<NodeHandle> {
        self.bones[bone.index()]
    }

    pub fn set(&mut self, bone: HumanBone, node: Option<NodeHandle>) {
        self.bones[bone.index()] = node;
    }

    /// The bones found, in [`HumanBone::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (HumanBone, NodeHandle)> + '_ {
        HumanBone::ALL
            .into_iter()
            .filter_map(|b| Some((b, self.get(b)?)))
    }

    /// Required bones that are not mapped.
    pub fn missing(&self) -> Vec<HumanBone> {
        HumanBone::ALL
            .into_iter()
            .filter(|b| b.is_required() && self.get(*b).is_none())
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.missing().is_empty()
    }

    /// Finds the humanoid bones among `bones` (a skin's joints, or every node of a
    /// skeleton): by name first, by the skeleton's shape for whatever names did not reveal.
    pub fn detect(tree: &SceneTree, bones: &[NodeHandle]) -> Self {
        let skeleton = Skeleton::of(tree, bones);
        let by_name = detect_by_name(&skeleton.names, &skeleton.parents);
        let mut found = by_name;
        if HumanBone::ALL
            .iter()
            .any(|b| b.is_required() && found[b.index()].is_none())
        {
            let by_shape = detect_by_shape(&skeleton.positions, &skeleton.parents);
            for bone in HumanBone::ALL {
                found[bone.index()] = found[bone.index()].or(by_shape[bone.index()]);
            }
            found = validated(found, &skeleton.parents);
        }
        let mut map = Self::default();
        for bone in HumanBone::ALL {
            map.set(bone, found[bone.index()].map(|i| bones[i]));
        }
        map
    }
}

/// Names, parents (within the list) and global positions of a list of bones.
struct Skeleton {
    names: Vec<String>,
    parents: Vec<Option<usize>>,
    positions: Vec<Vec3>,
}

impl Skeleton {
    fn of(tree: &SceneTree, bones: &[NodeHandle]) -> Self {
        let index = |node: NodeHandle| bones.iter().position(|&b| b == node);
        Self {
            names: bones
                .iter()
                .map(|&b| tree.name(b).map(|n| n.to_owned()).unwrap_or_default())
                .collect(),
            parents: bones
                .iter()
                .map(|&b| {
                    std::iter::successors(tree.parent(b), |&n| tree.parent(n)).find_map(index)
                })
                .collect(),
            positions: bones
                .iter()
                .map(|&b| {
                    tree.global_transform(b)
                        .map_or(Vec3::ZERO, |g| g.translation.into())
                })
                .collect(),
        }
    }
}

// =============================================================================================
// Detection by name
// =============================================================================================

/// Words that mark bones a ragdoll must not use: ends, helpers, fingers, toes, face...
const EXCLUDED: &[&str] = &[
    "end",
    "nub",
    "tip",
    "twist",
    "roll",
    "ik",
    "fk",
    "pole",
    "target",
    "ctrl",
    "control",
    "helper",
    "socket",
    "weapon",
    "prop",
    "attach",
    "finger",
    "thumb",
    "index",
    "middle",
    "ring",
    "pinky",
    "little",
    "toe",
    "toes",
    "ball",
    "eye",
    "jaw",
    "tongue",
    "teeth",
    "hair",
    "breast",
    "bust",
    "tail",
    "skirt",
    "cloth",
    "ear",
    "brow",
    "lid",
    "cheek",
    "lip",
    "sec",
    "adj",
    "mch",
    "wgt",
    "tweak",
    "pivot",
    "corrective",
    "jiggle",
    "dynamic",
    "dyn",
];

/// Words that say nothing about the bone (rig prefixes).
const NOISE: &[&str] = &[
    "mixamorig",
    "def",
    "org",
    "j",
    "bip",
    "c",
    "bone",
    "bn",
    "jnt",
    "jt",
    "joint",
    "armature",
    "rig",
    "skeleton",
    "cc",
    "base",
    "b",
    "sk",
    "deform",
    "valvebiped",
    "character",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// What a bone's name says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    Bone(HumanBone),
    /// A spine or chest bone (which one depends on the others).
    Torso,
}

/// Lower-case words of a bone name: namespaces dropped, camelCase and digits split.
fn words(name: &str) -> Vec<String> {
    let name = name.rsplit([':', '|']).next().unwrap_or(name);
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for c in name.chars() {
        let boundary = match previous {
            _ if !c.is_alphanumeric() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                previous = None;
                continue;
            }
            Some(p) => {
                (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic())
            }
            None => false,
        };
        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
        previous = Some(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn classify(name: &str) -> Option<Named> {
    let words = words(name);
    if words.iter().any(|w| EXCLUDED.contains(&w.as_str())) {
        return None;
    }
    let mut side = None;
    let mut rest = String::new();
    for word in &words {
        match word.as_str() {
            "l" | "left" | "lf" | "lft" => side = Some(Side::Left),
            "r" | "right" | "rt" | "rgt" => side = Some(Side::Right),
            w if NOISE.contains(&w) => {}
            w if w.chars().all(|c| c.is_ascii_digit()) => {}
            w => rest.push_str(w),
        }
    }
    use HumanBone as H;
    let sided = |left: HumanBone, right: HumanBone| match side? {
        Side::Left => Some(Named::Bone(left)),
        Side::Right => Some(Named::Bone(right)),
    };
    match rest.as_str() {
        "upperarm" | "uparm" | "arm" | "humerus" => sided(H::LeftUpperArm, H::RightUpperArm),
        "forearm" | "lowerarm" | "lowarm" | "elbow" => sided(H::LeftLowerArm, H::RightLowerArm),
        "hand" | "wrist" => sided(H::LeftHand, H::RightHand),
        // `Hip_L` is a leg; a plain `Hip` is the hips (below).
        "upleg" | "upperleg" | "thigh" | "femur" => sided(H::LeftUpperLeg, H::RightUpperLeg),
        "hip" if side.is_some() => sided(H::LeftUpperLeg, H::RightUpperLeg),
        "leg" | "lowerleg" | "lowleg" | "calf" | "shin" | "knee" | "crus" => {
            sided(H::LeftLowerLeg, H::RightLowerLeg)
        }
        "foot" | "ankle" => sided(H::LeftFoot, H::RightFoot),
        _ if side.is_some() => None,
        "hips" | "hip" | "pelvis" => Some(Named::Bone(H::Hips)),
        "neck" => Some(Named::Bone(H::Neck)),
        "head" => Some(Named::Bone(H::Head)),
        "spine" | "chest" | "upperchest" | "abdomen" | "torso" | "waist" | "stomach"
        | "ribcage" | "thorax" | "back" | "lowerback" | "upperback" => Some(Named::Torso),
        _ => None,
    }
}

/// Bone indices per [`HumanBone`] from names; `parents` gives each bone's nearest ancestor
/// in the list.
pub(crate) fn detect_by_name(
    names: &[String],
    parents: &[Option<usize>],
) -> [Option<usize>; HumanBone::COUNT] {
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = parents[i] {
            i = p;
            d += 1;
        }
        d
    };
    let named: Vec<Option<Named>> = names.iter().map(|n| classify(n)).collect();
    let mut found = [None; HumanBone::COUNT];
    // Each bone: the match nearest the root (`neck_01` before `neck_02`).
    for (i, kind) in named.iter().enumerate() {
        if let Some(Named::Bone(bone)) = kind {
            let slot = &mut found[bone.index()];
            if slot.is_none_or(|s| depth(i) < depth(s)) {
                *slot = Some(i);
            }
        }
    }
    // The torso: the spine is its first bone, the chest its last before the neck and arms.
    let mut torso: Vec<usize> = (0..names.len())
        .filter(|&i| named[i] == Some(Named::Torso))
        .collect();
    torso.sort_by_key(|&i| depth(i));
    if let Some(hips) = found[HumanBone::Hips.index()] {
        torso.retain(|&t| is_ancestor(parents, hips, t));
    } else if !torso.is_empty() {
        // No bone named hips: the first torso bone is the pelvis (`Bip01 Spine` rigs).
        found[HumanBone::Hips.index()] = Some(torso.remove(0));
    }
    if let Some(&spine) = torso.first() {
        found[HumanBone::Spine.index()] = Some(spine);
        let above = [HumanBone::Neck, HumanBone::Head, HumanBone::LeftUpperArm]
            .into_iter()
            .find_map(|b| found[b.index()]);
        let chest = torso
            .iter()
            .rev()
            .copied()
            .find(|&t| t != spine && above.is_none_or(|a| is_ancestor(parents, t, a)));
        found[HumanBone::Chest.index()] = chest;
    }
    validated(found, parents)
}

fn is_ancestor(parents: &[Option<usize>], ancestor: usize, mut node: usize) -> bool {
    while let Some(p) = parents[node] {
        if p == ancestor {
            return true;
        }
        node = p;
    }
    false
}

/// Drops bones that are not where their chain needs them (a lower leg that is not below
/// its upper leg, ...).
fn validated(
    mut found: [Option<usize>; HumanBone::COUNT],
    parents: &[Option<usize>],
) -> [Option<usize>; HumanBone::COUNT] {
    use HumanBone as H;
    const CHAINS: [[HumanBone; 2]; 14] = [
        [H::Hips, H::Spine],
        [H::Spine, H::Chest],
        [H::Hips, H::Head],
        [H::Neck, H::Head],
        [H::LeftUpperArm, H::LeftLowerArm],
        [H::LeftLowerArm, H::LeftHand],
        [H::RightUpperArm, H::RightLowerArm],
        [H::RightLowerArm, H::RightHand],
        [H::Hips, H::LeftUpperLeg],
        [H::LeftUpperLeg, H::LeftLowerLeg],
        [H::LeftLowerLeg, H::LeftFoot],
        [H::Hips, H::RightUpperLeg],
        [H::RightUpperLeg, H::RightLowerLeg],
        [H::RightLowerLeg, H::RightFoot],
    ];
    for [upper, lower] in CHAINS {
        if let (Some(a), Some(b)) = (found[upper.index()], found[lower.index()]) {
            if !is_ancestor(parents, a, b) {
                found[lower.index()] = None;
            }
        }
    }
    found
}

// =============================================================================================
// Detection by shape
// =============================================================================================

/// Bone indices per [`HumanBone`] from the skeleton's shape alone, for rigs whose names say
/// nothing (`Bone.001`): the hips are where the legs and the spine branch; the legs are the
/// two branches that go down, the spine the one that goes up; the arms branch off the spine.
/// The character is assumed to face +Z or -Z; left and right come from that.
pub(crate) fn detect_by_shape(
    positions: &[Vec3],
    parents: &[Option<usize>],
) -> [Option<usize>; HumanBone::COUNT] {
    use HumanBone as H;
    let mut found = [None; HumanBone::COUNT];
    let n = positions.len();
    let children = |i: usize| -> Vec<usize> { (0..n).filter(|&c| parents[c] == Some(i)).collect() };
    // A chain of bones down from `i`, taking at each step the child that `score`s highest.
    fn chain(
        i: usize,
        children: &dyn Fn(usize) -> Vec<usize>,
        score: &dyn Fn(usize) -> f32,
    ) -> Vec<usize> {
        let mut out = vec![i];
        let mut current = i;
        loop {
            let kids = children(current);
            let Some(&next) = kids.iter().max_by(|&&a, &&b| score(a).total_cmp(&score(b))) else {
                return out;
            };
            out.push(next);
            current = next;
        }
    }
    let subtree = |i: usize| (0..n).filter(move |&d| d == i || is_ancestor(parents, i, d));
    let size = |i: usize| subtree(i).count();
    let top = |i: usize| subtree(i).map(|d| positions[d].y).fold(f32::MIN, f32::max);
    let bottom = |i: usize| subtree(i).map(|d| positions[d].y).fold(f32::MAX, f32::min);
    // Hips: the bone with three branches of two or more bones (legs and spine) holding the
    // most bones (so not the chest, where neck and arms branch).
    let hips = (0..n)
        .filter(|&i| children(i).iter().filter(|&&c| size(c) >= 2).count() >= 3)
        .max_by_key(|&i| size(i));
    let Some(hips) = hips else {
        return found;
    };
    found[H::Hips.index()] = Some(hips);
    let up_of = |i: usize| positions[i].y;
    let mut branches: Vec<usize> = children(hips)
        .into_iter()
        .filter(|&c| size(c) >= 2)
        .collect();
    if branches.len() < 3 {
        return found;
    }
    // The spine reaches highest; the legs reach lowest.
    branches.sort_by(|&a, &b| top(a).total_cmp(&top(b)));
    let spine = chain(branches.pop().expect("three or more"), &children, &|c| {
        top(c)
    });
    branches.sort_by(|&a, &b| bottom(a).total_cmp(&bottom(b)));
    let downward = |c: usize| -bottom(c);
    let (leg_a, leg_b) = (
        chain(branches[0], &children, &downward),
        chain(branches[1], &children, &downward),
    );
    // glTF characters face +Z, which puts their left side at +X.
    let (left_leg, right_leg) = if positions[leg_a[0]].x >= positions[leg_b[0]].x {
        (&leg_a, &leg_b)
    } else {
        (&leg_b, &leg_a)
    };
    for (leg, [upper, lower, foot]) in [
        (left_leg, [H::LeftUpperLeg, H::LeftLowerLeg, H::LeftFoot]),
        (
            right_leg,
            [H::RightUpperLeg, H::RightLowerLeg, H::RightFoot],
        ),
    ] {
        // Skip zero-length bones at the hip joint.
        let start = leg
            .iter()
            .position(|&b| positions[b].distance(positions[hips]) > 1e-3)
            .unwrap_or(0);
        let leg = &leg[start..];
        let top = up_of(leg[0]);
        let bottom = leg.iter().map(|&b| up_of(b)).fold(f32::MAX, f32::min);
        let height = (top - bottom).max(1e-3);
        found[upper.index()] = Some(leg[0]);
        // The knee near half height, the ankle near the bottom.
        let knee = leg[1..].iter().copied().min_by(|&a, &b| {
            let mid = top - height * 0.5;
            (up_of(a) - mid).abs().total_cmp(&(up_of(b) - mid).abs())
        });
        found[lower.index()] = knee;
        found[foot.index()] = leg
            .iter()
            .copied()
            .find(|&b| up_of(b) < bottom + height * 0.15 && Some(b) != knee);
    }
    // Up the spine: the head at the top (the last bone still well above the shoulders), the
    // neck below it, the chest where the arms branch.
    let spine_set: Vec<usize> = spine.clone();
    found[H::Spine.index()] = spine_set.first().copied();
    let branching: Vec<usize> = spine_set
        .iter()
        .copied()
        .filter(|&s| {
            children(s)
                .iter()
                .any(|c| !spine_set.contains(c) && size(*c) >= 2)
        })
        .collect();
    if let Some(&chest) = branching.last() {
        if Some(chest) != found[H::Spine.index()] {
            found[H::Chest.index()] = Some(chest);
        }
        let arms: Vec<Vec<usize>> = children(chest)
            .into_iter()
            .filter(|c| !spine_set.contains(c))
            .map(|c| chain(c, &children, &|k| size(k) as f32))
            .filter(|c| c.len() >= 3)
            .collect();
        if arms.len() >= 2 {
            let (a, b) = (&arms[0], &arms[1]);
            let tip = |arm: &Vec<usize>| positions[*arm.last().unwrap()].x;
            let (left, right) = if tip(a) >= tip(b) { (a, b) } else { (b, a) };
            for (arm, [upper, lower, hand]) in [
                (left, [H::LeftUpperArm, H::LeftLowerArm, H::LeftHand]),
                (right, [H::RightUpperArm, H::RightLowerArm, H::RightHand]),
            ] {
                // A clavicle first when the chain is long enough for one.
                let start = usize::from(arm.len() >= 4);
                found[upper.index()] = arm.get(start).copied();
                found[lower.index()] = arm.get(start + 1).copied();
                found[hand.index()] = arm.get(start + 2).copied();
            }
        }
        let above: Vec<usize> = spine_set
            .iter()
            .copied()
            .skip_while(|&s| s != chest)
            .skip(1)
            .collect();
        match above.as_slice() {
            [] => {}
            [head] => found[H::Head.index()] = Some(*head),
            [neck, head, ..] => {
                found[H::Neck.index()] = Some(*neck);
                found[H::Head.index()] = Some(*head);
            }
        }
    }
    found
}

// =============================================================================================
// Building
// =============================================================================================

/// Settings of [`create_humanoid_ragdoll`].
#[derive(Debug, Clone, PartialEq)]
pub struct HumanoidRagdollSettings {
    /// Total mass in kg, spread like a person's.
    pub total_mass: f32,
    /// Scales the bodies' thickness (1 = average build).
    pub thickness: f32,
    /// Start limp instead of following the animation.
    pub active: bool,
    pub collision_layer: u32,
    pub collision_mask: u32,
}

impl Default for HumanoidRagdollSettings {
    fn default() -> Self {
        Self {
            total_mass: 70.0,
            thickness: 1.0,
            active: false,
            collision_layer: 1,
            collision_mask: 1,
        }
    }
}

/// Share of the body's mass (Winter's anthropometric tables).
fn mass_share(part: Part) -> f32 {
    match part {
        Part::Pelvis => 0.142,
        Part::Belly => 0.139,
        Part::Chest => 0.216,
        Part::Head => 0.081,
        Part::UpperArm(_) => 0.028,
        Part::LowerArm(_) => 0.022,
        Part::UpperLeg(_) => 0.1,
        Part::LowerLeg(_) => 0.0465,
        Part::Foot(_) => 0.0145,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Pelvis,
    Belly,
    Chest,
    Head,
    UpperArm(Side),
    LowerArm(Side),
    UpperLeg(Side),
    LowerLeg(Side),
    Foot(Side),
}

impl Part {
    fn name(self) -> String {
        let side = |s: Side| if s == Side::Left { "L" } else { "R" };
        match self {
            Self::Pelvis => "Pelvis".into(),
            Self::Belly => "Belly".into(),
            Self::Chest => "Chest".into(),
            Self::Head => "Head".into(),
            Self::UpperArm(s) => format!("UpperArm{}", side(s)),
            Self::LowerArm(s) => format!("LowerArm{}", side(s)),
            Self::UpperLeg(s) => format!("UpperLeg{}", side(s)),
            Self::LowerLeg(s) => format!("LowerLeg{}", side(s)),
            Self::Foot(s) => format!("Foot{}", side(s)),
        }
    }
}

/// One body planned: what it is, the bone it moves, its shape and where it sits.
struct Planned {
    part: Part,
    bone: NodeHandle,
    shape: Shape,
    global: Affine3A,
}

/// One joint planned: its parts, kind, and the global frame whose X axis is the joint's.
struct PlannedJoint {
    name: String,
    a: Part,
    b: Part,
    kind: JointKind,
    at: Vec3,
    axis: Vec3,
}

/// Builds a humanoid ragdoll under `parent` from `map` (see the module docs). The bones
/// must be posed (bind or rest pose) when this runs; their current global transforms set
/// the bodies' sizes.
pub fn create_humanoid_ragdoll(
    tree: &mut SceneTree,
    parent: NodeHandle,
    name: &str,
    map: &HumanoidMap,
    settings: &HumanoidRagdollSettings,
) -> Result<NodeHandle, PhysicsError> {
    if let Some(&missing) = map.missing().first() {
        return Err(PhysicsError::MissingHumanBone(missing.key()));
    }
    let root = tree.create_ragdoll(
        name,
        Ragdoll {
            active: settings.active,
        },
    );
    tree.add_child(parent, root)?;
    fill_humanoid_ragdoll(tree, root, map, settings)?;
    Ok(root)
}

/// Builds the bodies and joints of a humanoid ragdoll under an existing `Ragdoll` node
/// (`root`), as [`create_humanoid_ragdoll`] does.
pub fn fill_humanoid_ragdoll(
    tree: &mut SceneTree,
    root: NodeHandle,
    map: &HumanoidMap,
    settings: &HumanoidRagdollSettings,
) -> Result<(), PhysicsError> {
    if let Some(&missing) = map.missing().first() {
        return Err(PhysicsError::MissingHumanBone(missing.key()));
    }
    let (parts, joints) = plan(tree, map, settings);
    let kind = if settings.active {
        BodyKind::Rigid
    } else {
        BodyKind::Kinematic
    };
    let total_share: f32 = parts.iter().map(|p| mass_share(p.part)).sum();
    let mut nodes: Vec<(Part, NodeHandle)> = Vec::new();
    for planned in parts {
        let mut body = PhysicsBody3D::new(kind)
            .with_mass(settings.total_mass * mass_share(planned.part) / total_share)
            .with_friction(0.8)
            .with_collision(settings.collision_layer, settings.collision_mask);
        // A little damping keeps limp limbs from flailing.
        body.angular_damping = 0.6;
        body.linear_damping = 0.05;
        let node = tree.create_physics_body3d(&planned.part.name(), body);
        tree.add_collision_shape(node, planned.shape)?;
        tree.add_child(root, node)?;
        tree.set_global_transform(node, planned.global)?;
        let bone_global = tree
            .global_transform(planned.bone)
            .unwrap_or(Affine3A::IDENTITY);
        tree.insert_component(
            node,
            RagdollBone {
                bone: planned.bone,
                offset: planned.global.inverse() * bone_global,
            },
        )?;
        nodes.push((planned.part, node));
    }
    let node_of = |part: Part| nodes.iter().find(|(p, _)| *p == part).map(|(_, n)| *n);
    for joint in joints {
        let (Some(a), Some(b)) = (node_of(joint.a), node_of(joint.b)) else {
            continue;
        };
        let node = tree.create_joint3d(&joint.name, Joint3D::new(joint.kind, a, b));
        tree.add_child(root, node)?;
        tree.set_global_transform(
            node,
            Affine3A::from_rotation_translation(
                Quat::from_rotation_arc(Vec3::X, joint.axis.normalize_or(Vec3::X)),
                joint.at,
            ),
        )?;
    }
    Ok(())
}

/// Works out the bodies and joints from the bones' current positions.
fn plan(
    tree: &SceneTree,
    map: &HumanoidMap,
    settings: &HumanoidRagdollSettings,
) -> (Vec<Planned>, Vec<PlannedJoint>) {
    use HumanBone as H;
    let at = |bone: HumanBone| -> Option<Vec3> {
        tree.global_transform(map.get(bone)?)
            .map(|g| g.translation.into())
    };
    let req = |bone: HumanBone| at(bone).unwrap_or(Vec3::ZERO);
    let hips = req(H::Hips);
    let head = req(H::Head);
    let neck = at(H::Neck);
    let (left_leg, right_leg) = (req(H::LeftUpperLeg), req(H::RightUpperLeg));
    let (left_arm, right_arm) = (req(H::LeftUpperArm), req(H::RightUpperArm));

    // The body's frame: up the spine, right across the hips, forward from both.
    let up = (head - hips).normalize_or(Vec3::Y);
    let right_raw = right_leg - left_leg;
    let right = (right_raw - up * right_raw.dot(up)).normalize_or(Vec3::X);
    let forward = up.cross(right).normalize_or(Vec3::Z);
    // Rough height, for the thickness of everything.
    let lowest = [H::LeftFoot, H::RightFoot, H::LeftLowerLeg, H::RightLowerLeg]
        .into_iter()
        .filter_map(at)
        .map(|p| (p - hips).dot(up))
        .fold(0.0_f32, f32::min);
    let height = ((head - hips).dot(up) - lowest).max(0.3) * 1.1;
    let k = height * settings.thickness.max(0.1);

    let mut parts = Vec::new();
    // A box from `from` to `to` (its height), `width` across and `depth` front to back.
    let torso_box = |from: Vec3, to: Vec3, width: f32, depth: f32| {
        let axis = (to - from).normalize_or(up);
        let across = (right - axis * right.dot(axis)).normalize_or(right);
        // Right-handed (x × y = z), or the matrix is a mirror and not a rotation.
        let depth_axis = across.cross(axis);
        let rotation = Quat::from_mat3(&Mat3::from_cols(across, axis, depth_axis)).normalize();
        let length = from.distance(to).max(0.02 * k);
        (
            Shape::Box {
                size: Vec3::new(width, length, depth),
            },
            Affine3A::from_rotation_translation(rotation, (from + to) * 0.5),
        )
    };
    let capsule = |from: Vec3, to: Vec3, radius: f32| {
        let axis = (to - from).normalize_or(-up);
        let length = from.distance(to).max(1e-3);
        let radius = radius.min(length * 0.45);
        (
            Shape::Capsule {
                radius,
                height: length + radius * 2.0,
            },
            Affine3A::from_rotation_translation(
                Quat::from_rotation_arc(Vec3::Y, axis),
                (from + to) * 0.5,
            ),
        )
    };
    let hip_width = left_leg.distance(right_leg).max(0.08 * k);
    let shoulder_width = left_arm.distance(right_arm).max(0.15 * k);
    let spine = at(H::Spine);
    let chest = at(H::Chest);
    let top = neck.unwrap_or(head);
    // Pelvis: from a little below the hips to the spine (or a third of the way up).
    let pelvis_top = spine.unwrap_or(hips + (top - hips) * 0.3);
    let pelvis_bottom = hips - up * 0.05 * k;
    let (shape, global) = torso_box(
        pelvis_bottom,
        pelvis_top.max_along(hips + up * 0.06 * k, up),
        hip_width + 0.09 * k,
        0.12 * k,
    );
    parts.push(Planned {
        part: Part::Pelvis,
        bone: map.get(H::Hips).expect("required"),
        shape,
        global,
    });
    // Belly and chest; one torso box when the rig has a single spine bone.
    let mut upper_torso = Part::Chest;
    match (map.get(H::Spine), chest) {
        (Some(spine_bone), Some(chest_at)) => {
            let spine_at = spine.expect("mapped");
            let (shape, global) = torso_box(spine_at, chest_at, shoulder_width * 0.72, 0.13 * k);
            parts.push(Planned {
                part: Part::Belly,
                bone: spine_bone,
                shape,
                global,
            });
            let (shape, global) = torso_box(chest_at, top, shoulder_width * 0.85, 0.15 * k);
            parts.push(Planned {
                part: Part::Chest,
                bone: map.get(H::Chest).expect("mapped"),
                shape,
                global,
            });
        }
        (spine_bone, _) => {
            let from = spine.unwrap_or(pelvis_top);
            let (shape, global) = torso_box(from, top, shoulder_width * 0.8, 0.14 * k);
            let bone = spine_bone
                .or(map.get(H::Chest))
                .unwrap_or(map.get(H::Hips).expect("required"));
            if spine_bone.is_none() && map.get(H::Chest).is_none() {
                // Only hips: the pelvis is the whole torso.
                upper_torso = Part::Pelvis;
            } else {
                parts.push(Planned {
                    part: Part::Chest,
                    bone,
                    shape,
                    global,
                });
            }
        }
    }
    // Head: a sphere above the neck, moving the neck bone when there is one (the head bone
    // follows it).
    let radius = (top.distance(head) * 1.2).clamp(0.055 * k, 0.075 * k);
    parts.push(Planned {
        part: Part::Head,
        bone: map.get(H::Neck).or(map.get(H::Head)).expect("required"),
        shape: Shape::Sphere { radius },
        global: Affine3A::from_translation(head + up * radius * 0.8),
    });
    // Limbs.
    for side in [Side::Left, Side::Right] {
        let pick =
            |left: HumanBone, right: HumanBone| if side == Side::Left { left } else { right };
        let (upper_arm, lower_arm, hand) = (
            pick(H::LeftUpperArm, H::RightUpperArm),
            pick(H::LeftLowerArm, H::RightLowerArm),
            pick(H::LeftHand, H::RightHand),
        );
        let shoulder = req(upper_arm);
        let elbow = req(lower_arm);
        let wrist = at(hand).unwrap_or(elbow + (elbow - shoulder));
        let (shape, global) = capsule(shoulder, elbow, 0.032 * k);
        parts.push(Planned {
            part: Part::UpperArm(side),
            bone: map.get(upper_arm).expect("required"),
            shape,
            global,
        });
        // The forearm reaches on over the hand.
        let reach = wrist + (wrist - elbow).normalize_or(-up) * 0.06 * k;
        let (shape, global) = capsule(elbow, reach, 0.026 * k);
        parts.push(Planned {
            part: Part::LowerArm(side),
            bone: map.get(lower_arm).expect("required"),
            shape,
            global,
        });
        let (upper_leg, lower_leg, foot) = (
            pick(H::LeftUpperLeg, H::RightUpperLeg),
            pick(H::LeftLowerLeg, H::RightLowerLeg),
            pick(H::LeftFoot, H::RightFoot),
        );
        let hip = req(upper_leg);
        let knee = req(lower_leg);
        let ankle = at(foot).unwrap_or(knee + (knee - hip));
        let (shape, global) = capsule(hip, knee, 0.055 * k);
        parts.push(Planned {
            part: Part::UpperLeg(side),
            bone: map.get(upper_leg).expect("required"),
            shape,
            global,
        });
        let (shape, global) = capsule(knee, ankle, 0.04 * k);
        parts.push(Planned {
            part: Part::LowerLeg(side),
            bone: map.get(lower_leg).expect("required"),
            shape,
            global,
        });
        if let Some(foot_bone) = map.get(foot) {
            // Towards the toes: the foot bone's child if it has one, else straight ahead.
            let toe = tree
                .children(foot_bone)
                .into_iter()
                .find_map(|c| tree.global_transform(c))
                .map(|g| Vec3::from(g.translation))
                .filter(|t| t.distance(ankle) > 0.02 * k)
                .unwrap_or(ankle + forward * 0.12 * k);
            let along = toe - ankle;
            let flat = (along - up * along.dot(up)).normalize_or(forward);
            let length = along.length().max(0.08 * k) * 1.35;
            let foot_height = 0.05 * k;
            let rotation = Quat::from_mat3(&Mat3::from_cols(up.cross(flat), up, flat)).normalize();
            let center = ankle + flat * length * 0.3 - up * foot_height * 0.35;
            parts.push(Planned {
                part: Part::Foot(side),
                bone: foot_bone,
                shape: Shape::Box {
                    size: Vec3::new(0.055 * k, foot_height, length),
                },
                global: Affine3A::from_rotation_translation(rotation, center),
            });
        }
    }

    // Joints. Hinges bend about their X axis: `cross(limb, forward)` makes a positive angle
    // bend the limb forwards (elbows) and a negative one backwards (knees).
    let deg = |d: f32| d.to_radians();
    let cone = |swing: f32, twist: f32| JointKind::ConeTwist {
        swing: deg(swing),
        twist: deg(twist),
    };
    let present = |part: Part| parts.iter().any(|p| p.part == part);
    let mut joints = Vec::new();
    let mut joint = |name: &str, a: Part, b: Part, kind: JointKind, at: Vec3, axis: Vec3| {
        joints.push(PlannedJoint {
            name: name.into(),
            a,
            b,
            kind,
            at,
            axis,
        });
    };
    if present(Part::Belly) {
        joint(
            "Waist",
            Part::Pelvis,
            Part::Belly,
            cone(25.0, 20.0),
            spine.unwrap_or(hips),
            up,
        );
        joint(
            "Spine",
            Part::Belly,
            Part::Chest,
            cone(20.0, 15.0),
            chest.unwrap_or(hips),
            up,
        );
    } else if upper_torso == Part::Chest {
        joint(
            "Spine",
            Part::Pelvis,
            Part::Chest,
            cone(30.0, 25.0),
            spine.unwrap_or(hips),
            up,
        );
    }
    joint("Neck", upper_torso, Part::Head, cone(35.0, 40.0), top, up);
    for side in [Side::Left, Side::Right] {
        let s = if side == Side::Left { "L" } else { "R" };
        let pick =
            |left: HumanBone, right: HumanBone| if side == Side::Left { left } else { right };
        let shoulder = req(pick(H::LeftUpperArm, H::RightUpperArm));
        let elbow = req(pick(H::LeftLowerArm, H::RightLowerArm));
        let wrist = at(pick(H::LeftHand, H::RightHand)).unwrap_or(elbow * 2.0 - shoulder);
        let arm = (elbow - shoulder).normalize_or(-up);
        let forearm = (wrist - elbow).normalize_or(arm);
        joint(
            &format!("Shoulder{s}"),
            upper_torso,
            Part::UpperArm(side),
            cone(80.0, 45.0),
            shoulder,
            arm,
        );
        joint(
            &format!("Elbow{s}"),
            Part::UpperArm(side),
            Part::LowerArm(side),
            JointKind::Hinge {
                limits: Some([deg(0.0), deg(145.0)]),
            },
            elbow,
            hinge_axis(forearm, forward, up),
        );
        let hip = req(pick(H::LeftUpperLeg, H::RightUpperLeg));
        let knee = req(pick(H::LeftLowerLeg, H::RightLowerLeg));
        let ankle = at(pick(H::LeftFoot, H::RightFoot)).unwrap_or(knee * 2.0 - hip);
        let thigh = (knee - hip).normalize_or(-up);
        let shin = (ankle - knee).normalize_or(thigh);
        joint(
            &format!("Hip{s}"),
            Part::Pelvis,
            Part::UpperLeg(side),
            cone(60.0, 20.0),
            hip,
            thigh,
        );
        joint(
            &format!("Knee{s}"),
            Part::UpperLeg(side),
            Part::LowerLeg(side),
            JointKind::Hinge {
                limits: Some([deg(-145.0), deg(0.0)]),
            },
            knee,
            hinge_axis(shin, forward, up),
        );
        if present(Part::Foot(side)) {
            joint(
                &format!("Ankle{s}"),
                Part::LowerLeg(side),
                Part::Foot(side),
                cone(30.0, 10.0),
                ankle,
                shin,
            );
        }
    }
    (parts, joints)
}

/// The hinge axis for a limb along `limb`: across it, so that a positive angle swings it
/// towards `forward`.
fn hinge_axis(limb: Vec3, forward: Vec3, up: Vec3) -> Vec3 {
    let axis = limb.cross(forward);
    if axis.length_squared() > 1e-6 {
        axis.normalize()
    } else {
        // A limb pointing straight ahead: bend about the body's left-right axis.
        limb.cross(up).normalize_or(Vec3::X)
    }
}

trait MaxAlong {
    /// `self`, or `other` when it lies further along `axis`.
    fn max_along(self, other: Vec3, axis: Vec3) -> Vec3;
}

impl MaxAlong for Vec3 {
    fn max_along(self, other: Vec3, axis: Vec3) -> Vec3 {
        if other.dot(axis) > self.dot(axis) {
            other
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HumanBone as H;

    fn detect(rig: &[(&str, Option<usize>)]) -> Vec<(HumanBone, String)> {
        let names: Vec<String> = rig.iter().map(|(n, _)| (*n).to_owned()).collect();
        let parents: Vec<Option<usize>> = rig.iter().map(|(_, p)| *p).collect();
        let found = detect_by_name(&names, &parents);
        HumanBone::ALL
            .into_iter()
            .filter_map(|b| Some((b, names[found[b.index()]?].clone())))
            .collect()
    }

    fn bone(found: &[(HumanBone, String)], bone: HumanBone) -> Option<&str> {
        found
            .iter()
            .find(|(b, _)| *b == bone)
            .map(|(_, n)| n.as_str())
    }

    /// Builds a rig: `(name, parent name)` in parent-first order.
    fn rig<'a>(bones: &[(&'a str, &str)]) -> Vec<(&'a str, Option<usize>)> {
        bones
            .iter()
            .map(|(name, parent)| (*name, bones.iter().position(|(n, _)| n == parent)))
            .collect()
    }

    #[test]
    fn words_split_every_naming_style() {
        assert_eq!(words("mixamorig:LeftUpLeg"), ["left", "up", "leg"]);
        assert_eq!(words("thigh.L"), ["thigh", "l"]);
        assert_eq!(words("J_Bip_L_UpperArm"), ["j", "bip", "l", "upper", "arm"]);
        assert_eq!(words("Bip01 L Thigh"), ["bip", "01", "l", "thigh"]);
        assert_eq!(words("spine_03"), ["spine", "03"]);
        assert_eq!(
            words("DEF-upper_arm.L.001"),
            ["def", "upper", "arm", "l", "001"]
        );
    }

    #[test]
    fn mixamo() {
        let found = detect(&rig(&[
            ("mixamorig:Hips", ""),
            ("mixamorig:Spine", "mixamorig:Hips"),
            ("mixamorig:Spine1", "mixamorig:Spine"),
            ("mixamorig:Spine2", "mixamorig:Spine1"),
            ("mixamorig:Neck", "mixamorig:Spine2"),
            ("mixamorig:Head", "mixamorig:Neck"),
            ("mixamorig:HeadTop_End", "mixamorig:Head"),
            ("mixamorig:LeftShoulder", "mixamorig:Spine2"),
            ("mixamorig:LeftArm", "mixamorig:LeftShoulder"),
            ("mixamorig:LeftForeArm", "mixamorig:LeftArm"),
            ("mixamorig:LeftHand", "mixamorig:LeftForeArm"),
            ("mixamorig:LeftHandThumb1", "mixamorig:LeftHand"),
            ("mixamorig:RightShoulder", "mixamorig:Spine2"),
            ("mixamorig:RightArm", "mixamorig:RightShoulder"),
            ("mixamorig:RightForeArm", "mixamorig:RightArm"),
            ("mixamorig:RightHand", "mixamorig:RightForeArm"),
            ("mixamorig:LeftUpLeg", "mixamorig:Hips"),
            ("mixamorig:LeftLeg", "mixamorig:LeftUpLeg"),
            ("mixamorig:LeftFoot", "mixamorig:LeftLeg"),
            ("mixamorig:LeftToeBase", "mixamorig:LeftFoot"),
            ("mixamorig:RightUpLeg", "mixamorig:Hips"),
            ("mixamorig:RightLeg", "mixamorig:RightUpLeg"),
            ("mixamorig:RightFoot", "mixamorig:RightLeg"),
        ]));
        assert_eq!(found.len(), 17, "{found:?}");
        assert_eq!(bone(&found, H::Spine), Some("mixamorig:Spine"));
        assert_eq!(bone(&found, H::Chest), Some("mixamorig:Spine2"));
        assert_eq!(bone(&found, H::LeftUpperArm), Some("mixamorig:LeftArm"));
        assert_eq!(bone(&found, H::RightLowerLeg), Some("mixamorig:RightLeg"));
        assert_eq!(bone(&found, H::Head), Some("mixamorig:Head"));
    }

    #[test]
    fn unreal() {
        let found = detect(&rig(&[
            ("root", ""),
            ("pelvis", "root"),
            ("spine_01", "pelvis"),
            ("spine_02", "spine_01"),
            ("spine_03", "spine_02"),
            ("neck_01", "spine_03"),
            ("head", "neck_01"),
            ("clavicle_l", "spine_03"),
            ("upperarm_l", "clavicle_l"),
            ("upperarm_twist_01_l", "upperarm_l"),
            ("lowerarm_l", "upperarm_l"),
            ("hand_l", "lowerarm_l"),
            ("index_01_l", "hand_l"),
            ("clavicle_r", "spine_03"),
            ("upperarm_r", "clavicle_r"),
            ("lowerarm_r", "upperarm_r"),
            ("hand_r", "lowerarm_r"),
            ("thigh_l", "pelvis"),
            ("thigh_twist_01_l", "thigh_l"),
            ("calf_l", "thigh_l"),
            ("foot_l", "calf_l"),
            ("ball_l", "foot_l"),
            ("thigh_r", "pelvis"),
            ("calf_r", "thigh_r"),
            ("foot_r", "calf_r"),
            ("ik_foot_root", "root"),
            ("ik_foot_l", "ik_foot_root"),
        ]));
        assert_eq!(found.len(), 17, "{found:?}");
        assert_eq!(bone(&found, H::Hips), Some("pelvis"));
        assert_eq!(bone(&found, H::Chest), Some("spine_03"));
        assert_eq!(bone(&found, H::LeftUpperArm), Some("upperarm_l"));
        assert_eq!(bone(&found, H::LeftFoot), Some("foot_l"), "not the IK bone");
    }

    #[test]
    fn blender_vroid_and_biped() {
        let blender = detect(&rig(&[
            ("hips", ""),
            ("spine", "hips"),
            ("chest", "spine"),
            ("neck", "chest"),
            ("head", "neck"),
            ("upper_arm.L", "chest"),
            ("forearm.L", "upper_arm.L"),
            ("hand.L", "forearm.L"),
            ("upper_arm.R", "chest"),
            ("forearm.R", "upper_arm.R"),
            ("hand.R", "forearm.R"),
            ("thigh.L", "hips"),
            ("shin.L", "thigh.L"),
            ("foot.L", "shin.L"),
            ("thigh.R", "hips"),
            ("shin.R", "thigh.R"),
            ("foot.R", "shin.R"),
        ]));
        assert_eq!(blender.len(), 17, "{blender:?}");

        let vroid = detect(&rig(&[
            ("J_Bip_C_Hips", ""),
            ("J_Bip_C_Spine", "J_Bip_C_Hips"),
            ("J_Bip_C_Chest", "J_Bip_C_Spine"),
            ("J_Bip_C_UpperChest", "J_Bip_C_Chest"),
            ("J_Bip_C_Neck", "J_Bip_C_UpperChest"),
            ("J_Bip_C_Head", "J_Bip_C_Neck"),
            ("J_Adj_L_FaceEye", "J_Bip_C_Head"),
            ("J_Bip_L_Shoulder", "J_Bip_C_UpperChest"),
            ("J_Bip_L_UpperArm", "J_Bip_L_Shoulder"),
            ("J_Bip_L_LowerArm", "J_Bip_L_UpperArm"),
            ("J_Bip_L_Hand", "J_Bip_L_LowerArm"),
            ("J_Bip_R_Shoulder", "J_Bip_C_UpperChest"),
            ("J_Bip_R_UpperArm", "J_Bip_R_Shoulder"),
            ("J_Bip_R_LowerArm", "J_Bip_R_UpperArm"),
            ("J_Bip_R_Hand", "J_Bip_R_LowerArm"),
            ("J_Bip_L_UpperLeg", "J_Bip_C_Hips"),
            ("J_Bip_L_LowerLeg", "J_Bip_L_UpperLeg"),
            ("J_Bip_L_Foot", "J_Bip_L_LowerLeg"),
            ("J_Bip_R_UpperLeg", "J_Bip_C_Hips"),
            ("J_Bip_R_LowerLeg", "J_Bip_R_UpperLeg"),
            ("J_Bip_R_Foot", "J_Bip_R_LowerLeg"),
            ("J_Sec_Hair1_01", "J_Bip_C_Head"),
        ]));
        assert_eq!(vroid.len(), 17, "{vroid:?}");
        assert_eq!(bone(&vroid, H::Chest), Some("J_Bip_C_UpperChest"));

        let biped = detect(&rig(&[
            ("Bip01", ""),
            ("Bip01 Pelvis", "Bip01"),
            ("Bip01 Spine", "Bip01 Pelvis"),
            ("Bip01 Spine1", "Bip01 Spine"),
            ("Bip01 Neck", "Bip01 Spine1"),
            ("Bip01 Head", "Bip01 Neck"),
            ("Bip01 L Clavicle", "Bip01 Neck"),
            ("Bip01 L UpperArm", "Bip01 L Clavicle"),
            ("Bip01 L Forearm", "Bip01 L UpperArm"),
            ("Bip01 L Hand", "Bip01 L Forearm"),
            ("Bip01 R Clavicle", "Bip01 Neck"),
            ("Bip01 R UpperArm", "Bip01 R Clavicle"),
            ("Bip01 R Forearm", "Bip01 R UpperArm"),
            ("Bip01 R Hand", "Bip01 R Forearm"),
            ("Bip01 L Thigh", "Bip01 Spine"),
            ("Bip01 L Calf", "Bip01 L Thigh"),
            ("Bip01 L Foot", "Bip01 L Calf"),
            ("Bip01 R Thigh", "Bip01 Spine"),
            ("Bip01 R Calf", "Bip01 R Thigh"),
            ("Bip01 R Foot", "Bip01 R Calf"),
        ]));
        assert!(biped.len() >= 15, "{biped:?}");
        assert_eq!(bone(&biped, H::Hips), Some("Bip01 Pelvis"));
        assert_eq!(bone(&biped, H::LeftUpperLeg), Some("Bip01 L Thigh"));
    }

    #[test]
    fn a_rig_with_meaningless_names_is_read_from_its_shape() {
        // A T-pose facing +Z, left at +X: hips at 1 m.
        let bones: Vec<(&str, &str, Vec3)> = vec![
            ("Bone", "", Vec3::new(0.0, 1.0, 0.0)),
            ("Bone.001", "Bone", Vec3::new(0.0, 1.2, 0.0)),
            ("Bone.002", "Bone.001", Vec3::new(0.0, 1.45, 0.0)),
            ("Bone.003", "Bone.002", Vec3::new(0.0, 1.55, 0.0)),
            ("Bone.004", "Bone.003", Vec3::new(0.0, 1.65, 0.0)),
            ("Bone.005", "Bone.004", Vec3::new(0.0, 1.85, 0.0)),
            ("Bone.006", "Bone.002", Vec3::new(0.1, 1.45, 0.0)),
            ("Bone.007", "Bone.006", Vec3::new(0.2, 1.45, 0.0)),
            ("Bone.008", "Bone.007", Vec3::new(0.5, 1.45, 0.0)),
            ("Bone.009", "Bone.008", Vec3::new(0.75, 1.45, 0.0)),
            ("Bone.010", "Bone.002", Vec3::new(-0.1, 1.45, 0.0)),
            ("Bone.011", "Bone.010", Vec3::new(-0.2, 1.45, 0.0)),
            ("Bone.012", "Bone.011", Vec3::new(-0.5, 1.45, 0.0)),
            ("Bone.013", "Bone.012", Vec3::new(-0.75, 1.45, 0.0)),
            ("Bone.014", "Bone", Vec3::new(0.1, 0.95, 0.0)),
            ("Bone.015", "Bone.014", Vec3::new(0.1, 0.5, 0.0)),
            ("Bone.016", "Bone.015", Vec3::new(0.1, 0.08, 0.0)),
            ("Bone.017", "Bone.016", Vec3::new(0.1, 0.0, 0.15)),
            ("Bone.018", "Bone", Vec3::new(-0.1, 0.95, 0.0)),
            ("Bone.019", "Bone.018", Vec3::new(-0.1, 0.5, 0.0)),
            ("Bone.020", "Bone.019", Vec3::new(-0.1, 0.08, 0.0)),
            ("Bone.021", "Bone.020", Vec3::new(-0.1, 0.0, 0.15)),
        ];
        let names: Vec<String> = bones.iter().map(|b| b.0.to_owned()).collect();
        let parents: Vec<Option<usize>> = bones
            .iter()
            .map(|b| bones.iter().position(|o| o.0 == b.1))
            .collect();
        let positions: Vec<Vec3> = bones.iter().map(|b| b.2).collect();
        assert!(detect_by_name(&names, &parents).iter().all(Option::is_none));
        let found = detect_by_shape(&positions, &parents);
        let name = |b: HumanBone| found[b.index()].map(|i| names[i].as_str());
        assert_eq!(name(H::Hips), Some("Bone"));
        assert_eq!(name(H::Chest), Some("Bone.002"));
        assert_eq!(name(H::Head), Some("Bone.004"));
        assert_eq!(name(H::Neck), Some("Bone.003"));
        assert_eq!(
            name(H::LeftUpperArm),
            Some("Bone.007"),
            "after the clavicle"
        );
        assert_eq!(name(H::RightLowerArm), Some("Bone.012"));
        assert_eq!(name(H::LeftUpperLeg), Some("Bone.014"));
        assert_eq!(name(H::LeftLowerLeg), Some("Bone.015"));
        assert_eq!(name(H::RightFoot), Some("Bone.020"));
    }

    #[test]
    fn misplaced_bones_are_dropped() {
        // A "calf" that is not under the thigh is not the lower leg.
        let found = detect(&rig(&[
            ("pelvis", ""),
            ("thigh_l", "pelvis"),
            ("calf_l", "pelvis"),
        ]));
        assert_eq!(bone(&found, H::LeftUpperLeg), Some("thigh_l"));
        assert_eq!(bone(&found, H::LeftLowerLeg), None);
    }

    #[test]
    fn hinges_bend_the_right_way() {
        // Arm hanging down, character facing +Z: a positive rotation about the elbow's axis
        // takes the forearm forwards.
        let axis = hinge_axis(Vec3::NEG_Y, Vec3::Z, Vec3::Y);
        let bent = Quat::from_axis_angle(axis, 0.5) * Vec3::NEG_Y;
        assert!(bent.z > 0.0, "{bent}");
    }

    #[test]
    fn keys_round_trip() {
        for bone in HumanBone::ALL {
            assert_eq!(HumanBone::from_key(bone.key()), Some(bone));
        }
        assert_eq!(H::LeftUpperArm.label(), "Left upper arm");
    }
}
