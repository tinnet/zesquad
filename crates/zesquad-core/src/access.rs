//! Who may do what with a squad: one rule, from the squad's visibility and your place in it.
//!
//! Every squad route asks [`allows`] before doing anything, so the rule lives here and nowhere
//! else. With accounts off, every squad is treated as [`Visibility::Open`].

use std::fmt;
use std::str::FromStr;

/// Who can see a squad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    /// From before accounts: anyone with the link sees and edits it, and nobody owns it.
    Open,
    /// Only members see it.
    Private,
    /// Anyone with the link can look; members edit.
    Unlisted,
    /// Like unlisted, and listed for everyone to browse.
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown visibility {0:?}")]
pub struct VisibilityError(String);

impl Visibility {
    pub const ALL: [Self; 4] = [Self::Open, Self::Private, Self::Unlisted, Self::Public];
    /// What an owner can pick; `Open` is only for squads from before accounts.
    pub const CHOICES: [Self; 3] = [Self::Private, Self::Unlisted, Self::Public];

    /// As stored in the database and sent by forms.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Private => "private",
            Self::Unlisted => "unlisted",
            Self::Public => "public",
        }
    }

    /// For people choosing: who can see the squad.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Anyone with the link, and they can edit it",
            Self::Private => "Only members",
            Self::Unlisted => "Anyone with the link",
            Self::Public => "Everyone",
        }
    }
}

impl FromStr for Visibility {
    type Err = VisibilityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| VisibilityError(s.to_owned()))
    }
}

/// For reading it from the database.
impl TryFrom<String> for Visibility {
    type Error = VisibilityError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Your place in a squad; not being in it is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Owner,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown role {0:?}")]
pub struct RoleError(String);

impl Role {
    pub const ALL: [Self; 2] = [Self::Owner, Self::Member];

    /// As stored in the database.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
        }
    }
}

impl FromStr for Role {
    type Err = RoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|r| r.as_str() == s)
            .ok_or_else(|| RoleError(s.to_owned()))
    }
}

impl TryFrom<String> for Role {
    type Error = RoleError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Something you might want to do with a squad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// See the squad and its characters.
    View,
    /// Add, remove, tag and refresh characters; pick tag colours.
    Edit,
    /// Rename it and choose its Discord channel: the owner's, or anyone's for an open squad.
    Configure,
    /// See the invite link and the members.
    SeeMembers,
    /// Change who can see it, reset the invite link, remove members, delete it.
    Own,
}

impl Action {
    pub const ALL: [Self; 5] = [
        Self::View,
        Self::Edit,
        Self::Configure,
        Self::SeeMembers,
        Self::Own,
    ];
}

/// Whether `role` (`None`: not a member, or not logged in) may do `action` with a squad that
/// has `visibility`.
#[must_use]
pub fn allows(visibility: Visibility, role: Option<Role>, action: Action) -> bool {
    match role {
        Some(Role::Owner) => true,
        Some(Role::Member) => matches!(action, Action::View | Action::Edit | Action::SeeMembers),
        None => match action {
            Action::View => visibility != Visibility::Private,
            // Squads from before accounts stay as they were: everything but owning.
            Action::Edit | Action::Configure => visibility == Visibility::Open,
            Action::SeeMembers | Action::Own => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_table() {
        use Action::{Configure, Edit, SeeMembers, View};
        use Visibility::{Open, Private, Public, Unlisted};
        const MEMBER: &[Action] = &[View, Edit, SeeMembers];
        // (visibility, role, what's allowed)
        let table: [(Visibility, Option<Role>, &[Action]); 12] = [
            (Open, Some(Role::Owner), &Action::ALL),
            (Open, Some(Role::Member), MEMBER),
            (Open, None, &[View, Edit, Configure]),
            (Private, Some(Role::Owner), &Action::ALL),
            (Private, Some(Role::Member), MEMBER),
            (Private, None, &[]),
            (Unlisted, Some(Role::Owner), &Action::ALL),
            (Unlisted, Some(Role::Member), MEMBER),
            (Unlisted, None, &[View]),
            (Public, Some(Role::Owner), &Action::ALL),
            (Public, Some(Role::Member), MEMBER),
            (Public, None, &[View]),
        ];
        for (visibility, role, allowed) in table {
            for action in Action::ALL {
                assert_eq!(
                    allows(visibility, role, action),
                    allowed.contains(&action),
                    "{visibility} {role:?} {action:?}"
                );
            }
        }
    }

    #[test]
    fn names_round_trip() {
        for v in Visibility::ALL {
            assert_eq!(v.as_str().parse::<Visibility>(), Ok(v));
        }
        for r in Role::ALL {
            assert_eq!(r.as_str().parse::<Role>(), Ok(r));
        }
        assert!("Public".parse::<Visibility>().is_err());
        assert!("admin".parse::<Role>().is_err());
        assert!(!Visibility::CHOICES.contains(&Visibility::Open));
    }

    fn visibility() -> impl Strategy<Value = Visibility> {
        prop::sample::select(Visibility::ALL.to_vec())
    }

    fn role() -> impl Strategy<Value = Option<Role>> {
        prop::option::of(prop::sample::select(Role::ALL.to_vec()))
    }

    fn action() -> impl Strategy<Value = Action> {
        prop::sample::select(Action::ALL.to_vec())
    }

    proptest! {
        #[test]
        fn doing_anything_implies_viewing(v in visibility(), r in role(), a in action()) {
            prop_assert!(!allows(v, r, a) || allows(v, r, Action::View));
        }

        #[test]
        fn editing_implies_viewing(v in visibility(), r in role()) {
            prop_assert!(!allows(v, r, Action::Edit) || allows(v, r, Action::View));
        }

        #[test]
        fn a_private_squad_shows_nothing_to_a_non_member(a in action()) {
            prop_assert!(!allows(Visibility::Private, None, a));
        }

        #[test]
        fn only_the_owner_owns(v in visibility(), r in role()) {
            prop_assert_eq!(allows(v, r, Action::Own), r == Some(Role::Owner));
        }

        #[test]
        fn members_see_and_edit_whatever_the_visibility(v in visibility(), r in role()) {
            if r.is_some() {
                prop_assert!(allows(v, r, Action::Edit) && allows(v, r, Action::SeeMembers));
            }
        }

        #[test]
        fn only_the_owner_configures_an_owned_squad(v in visibility(), r in role()) {
            if v != Visibility::Open {
                prop_assert_eq!(allows(v, r, Action::Configure), r == Some(Role::Owner));
            }
        }

        #[test]
        fn outsiders_never_see_members(v in visibility()) {
            prop_assert!(!allows(v, None, Action::SeeMembers));
        }
    }
}
