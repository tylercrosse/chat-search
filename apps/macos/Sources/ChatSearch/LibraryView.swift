import CsTheme
import SwiftUI

/// The second view, and the only one that is not derived from the index.
///
/// **Nothing constructs it. `chat-search-me9.8.30` scrapped the tab, "for now" in the markup's own
/// word, and this file is deliberately still in the target.** Compiled and unreachable is the
/// point: it goes on typechecking against the token seam, so a direction that adds a token or a
/// `Theme` API that moves breaks it here rather than in six months when somebody wants the shelves
/// back. Excluding it from `Package.swift` would have been the same deletion with a longer fuse.
/// `chat-search-me9.8.4` built it and git is not where you go looking for a surface you may want
/// next month.
///
/// What it was competing for was 30 points of vertical: the Search/Library switch was a strip
/// above the search bar on a window whose floor is 480, and the switch was the only thing in it.
/// Bringing it back means deciding where it goes, not reverting a commit — a menu item and ⇧⌘L
/// rather than a tab strip is the obvious answer, and `library.db` (`chat-search-6eb.14`) is the
/// thing that would make it worth reaching for at all.
///
/// Everything in Search survives `rm index.db && cs index` because none of it is anything but a
/// projection of the archive. Nothing here would: a collection, a pin, a project merge, a
/// dismissal are all things a person said, and ADR 3 puts them in `library.db` rather than in the
/// index for exactly that reason. That is why Collections kept failing to find a tab in the
/// prototype — it was the only authored thing on screen, competing with derived views for space
/// (`poc/ui/NOTES.md` §2).
///
/// **It is empty, and it says so in four different ways.** Nothing has been authored because there
/// is nowhere to author it: `library.db` is `chat-search-6eb.14` and is not built. Each shelf
/// below states what it will hold, what it is waiting on, and which bead that is — which is the
/// prototype's own reading of this view ("mostly empty, and it says so; these are the first empty
/// states in the prototype") rather than a placeholder screen.
///
/// Nothing here names a colour, a size or a face; every one is a token off `\.theme`
/// (`chat-search-me9.8.8`).
struct LibraryView: View {
    @Environment(\.theme) private var theme

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 26) {
                lead

                shelf(
                    "Collections", "a rule, evaluated — not a list",
                    empty: "No collections yet",
                    why: "A collection is stored as matches(query) + pinned − excluded, so it goes "
                        + "on answering after a reindex instead of freezing one answer. Making one "
                        + "means writing something down, and there is nowhere to write it yet.")

                shelf(
                    "Proposed", "seeded topics · being wrong costs one dismissal",
                    empty: "Nothing to propose",
                    why: "The prototype proposes collections from a seeded taxonomy — 26 seeds, "
                        + "37% of the corpus matching none of them, which is why a topic grouping "
                        + "needs a residue group. Topics are derived by poc/ui/export.py and are "
                        + "not an index fact, so nothing on this wire carries them. "
                        + "chat-search-me9.18 asks whether the grammar should; chat-search-6eb.18 "
                        + "is the discovery half.")

                shelf(
                    "Project merges", "a path is evidence, not identity — authored, never derived",
                    empty: "No merges to suggest",
                    why: "Two projects whose last path segment matches are usually one project "
                        + "that moved: /dev/career (89) + /dev/projects/career (65) = 154. Finding "
                        + "that pair needs a corpus-true list of directories and cs facets carries "
                        + "sources only (chat-search-1ld). Suggested and never applied in any "
                        + "case — the merge is an authored fact, and guessing it into the index "
                        + "would make a derived column depend on a judgement call.")

                shelf(
                    "Pinned", "kept by hand",
                    empty: "Nothing pinned",
                    why: "A pin puts a conversation in a collection the rule would not have "
                        + "found. It is the escape hatch that lets the rule stay simple, and it is "
                        + "the plainest thing on this screen that a rebuild would destroy if it "
                        + "were kept in the index.")
            }
            .padding(.horizontal, 20)
            .padding(.top, 16)
            .padding(.bottom, 40)
            // The prototype's `max-width: 940px`, and then pushed left rather than centred: a
            // shelf that will hold rows should start where its first row would.
            .frame(maxWidth: 940, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        // The bead ids in this view are the useful part of it: they are what somebody reading an
        // empty shelf would want to go and look up.
        .textSelection(.enabled)
        .background(theme.color(.bg))
    }

    private var lead: some View {
        Text(
            "Everything else in this window is derived from the index and can be thrown away with "
                + "it — rm index.db && cs index is a valid answer to any schema change. This is "
                + "the half that cannot be: what you pinned, named, merged or dismissed. It is "
                + "empty because it has nowhere to live yet — authored data belongs in library.db "
                + "and not in the index (ADR 3), which is chat-search-6eb.14."
        )
        .font(theme.font(.body))
        .foregroundStyle(theme.color(.ink2))
        .lineSpacing(3)
        .frame(maxWidth: 620, alignment: .leading)
    }

    /// `poc/ui`'s `.lib-sec`: a heading and, beside it, the rule the shelf keeps.
    private func shelf(_ title: String, _ note: String, empty: String, why: String) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(title)
                    .font(theme.font(.head, weight: .semibold))
                    .foregroundStyle(theme.color(.ink))
                Text(note)
                    .font(theme.font(.meta, .mono))
                    .foregroundStyle(theme.color(.ink3))
            }
            state(empty, why)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// The mockup's `.empty`, in its Library form: left-aligned rather than centred, because a
    /// shelf that will hold rows should say so where its first row would be.
    private func state(_ what: String, _ why: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(what)
                .font(theme.font(.body, weight: .semibold))
                .foregroundStyle(theme.color(.ink2))
            Text(why)
                .font(theme.font(.body))
                .foregroundStyle(theme.color(.ink3))
                .lineSpacing(3)
                .frame(maxWidth: 620, alignment: .leading)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(
            RoundedRectangle(cornerRadius: theme.metric(.r6))
                .strokeBorder(theme.color(.rule2), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
    }
}
