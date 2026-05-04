#!/usr/bin/env bash
set -euo pipefail

# ── setup-zfb-wisdom.sh ────────────────────────────────────
# Creates a Claude Code skill (zfb-wisdom) that exposes the
# zfb project documentation as a browsable knowledge base,
# then symlinks it into the user-scope skills directory
# (~/.claude/skills/).
#
# Run from anywhere inside the repo:
#   bash docs/scripts/setup-zfb-wisdom.sh
# ────────────────────────────────────────────────────────────

SKILL_NAME="zfb-wisdom"

# Resolve docs sub-project root (the directory containing this script's parent)
DOCS_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Resolve the main repo root (handles git worktrees: always points at the main
# worktree so symlinks remain valid after a worktree is removed)
REPO_ROOT="$(git -C "$DOCS_DIR" worktree list | head -1 | awk '{print $1}')"
if [ -z "$REPO_ROOT" ]; then
  echo "Error: Could not resolve repo root (git worktree list returned empty)"
  exit 1
fi

SKILL_DIR="$DOCS_DIR/.claude/skills/$SKILL_NAME"
DOCS_CONTENT_DIR="$DOCS_DIR/src/content/docs"
DOCS_JA_CONTENT_DIR="$DOCS_DIR/src/content/docs-ja"
ARCHITECTURE_DIR="$REPO_ROOT/docs/architecture"
GLOBAL_SKILLS_DIR="$HOME/.claude/skills"

echo ""
echo "=== zfb-wisdom Skill Setup ==="
echo ""

# Validate docs content directory exists
if [ ! -d "$DOCS_CONTENT_DIR" ]; then
  echo "Error: Documentation directory not found at $DOCS_CONTENT_DIR"
  exit 1
fi

# Helper: replace a symlink or file at the given path with a new symlink
ensure_symlink() {
  local link_path="$1"
  local target="$2"
  if [ -L "$link_path" ] || [ -e "$link_path" ]; then
    rm -rf "$link_path"
  fi
  ln -s "$target" "$link_path"
  echo "  Symlinked $link_path -> $target"
}

# Create skill directory
mkdir -p "$SKILL_DIR"
echo "  Created skill directory: $SKILL_DIR"

# Symlink docs/ → docs/src/content/docs/
ensure_symlink "$SKILL_DIR/docs" "$REPO_ROOT/docs/src/content/docs"

# Symlink docs-ja/ → docs/src/content/docs-ja/ (if it exists)
HAS_JA=""
if [ -d "$DOCS_JA_CONTENT_DIR" ]; then
  HAS_JA="true"
  ensure_symlink "$SKILL_DIR/docs-ja" "$REPO_ROOT/docs/src/content/docs-ja"
fi

# Symlink architecture/ → docs/architecture/ (ADR directory)
if [ -d "$ARCHITECTURE_DIR" ]; then
  ensure_symlink "$SKILL_DIR/architecture" "$ARCHITECTURE_DIR"
fi

# Discover top-level doc categories dynamically for the SKILL.md index
DOC_TREE=""
for dir in "$DOCS_CONTENT_DIR"/*/; do
  [ -d "$dir" ] || continue
  dirname="$(basename "$dir")"
  DOC_TREE="${DOC_TREE}- ${dirname}/
"
done

# Generate SKILL.md with YAML frontmatter + category index
cat > "$SKILL_DIR/SKILL.md" << SKILLEOF
---
name: $SKILL_NAME
description: >-
  Search and reference documentation from the zfb (zudo-front-builder) project.
  Use when answering questions about zfb features, CLI usage, configuration,
  content collections, routing, build pipeline, or architecture decisions.
user-invocable: true
argument-hint: "[-u|--update] [topic keyword, e.g., 'routing', 'build-pipeline', 'content-collections']"
---

# zfb Documentation Reference

Look up documentation from the zfb (zudo-front-builder) project.
Documentation base path: \`docs/src/content/docs\` (relative to repo root)

## Mode Detection

Parse the argument string for flags:

- If args start with \`-u\` or \`--update\`: enter **Update mode** (see below)
- Otherwise: enter **Lookup mode** (default)

Strip the flag from the remaining argument to get the topic keyword.

## Lookup Mode (default)

1. Find the relevant article(s) from the \`docs/\` directory based on the topic
2. Read ONLY the specific article(s) you need — do NOT load all articles at once
3. For architecture questions, also check \`architecture/\` for ADR files
4. Apply the information from the article when answering the user's question
5. Mention the source article path so the user can find it for further reading

## Update Mode (\`-u\` / \`--update\`)

The user has new information and wants to add or update documentation in this repo.

### Workflow

1. **Understand the new info**: Ask the user what they learned or want to
   document. The topic keyword (if provided) hints at the subject area.
2. **Find existing docs**: Search the \`docs/\` directory for articles related to
   the topic. Read them to understand what is already covered.
3. **Decide create vs update**: If an existing article covers the topic, update
   it. Otherwise, create a new \`.mdx\` file in the appropriate subdirectory.
4. **Write the content**: Follow the doc-authoring rules in \`docs/CLAUDE.md\`:
   - Required frontmatter: \`title\` (string). Always set \`sidebar_position\`.
     Optional: \`description\`, \`sidebar_label\`, \`tags\`, etc.
   - Do NOT use \`# h1\` in content — the frontmatter \`title\` renders as h1.
     Start with \`## h2\` headings.
   - Use available MDX components (\`<Note>\`, \`<Tip>\`, \`<Info>\`, \`<Warning>\`,
     \`<Danger>\`) where appropriate.
5. **Update Japanese docs**: Create or update the corresponding file under
   \`docs-ja/\` mirroring the English directory structure. Keep code blocks and
   diagrams identical — only translate surrounding prose.
6. **Format**: Run \`pnpm format:md\` inside \`docs/\` to format the changed files.
7. **Verify**: Run \`pnpm build\` inside \`docs/\` to confirm the site builds correctly.

## Documentation Structure

The documentation is organized in MDX files under \`docs/\`:

\`\`\`
${DOC_TREE}\`\`\`

Architecture Decision Records (ADRs) are in \`architecture/\`.

Browse the \`docs/\` directory to discover available articles. Each \`.mdx\` file
has YAML frontmatter with \`title\` and \`description\` fields that help identify
the right article to read.
SKILLEOF

if [ "$HAS_JA" = "true" ]; then
  cat >> "$SKILL_DIR/SKILL.md" << JAEOF

## Japanese Documentation

Japanese translations are available under \`docs-ja/\`. When the user is working
in Japanese or asks for Japanese content, prefer articles from \`docs-ja/\`.
JAEOF
fi

echo "  Generated SKILL.md"

# Symlink into global skills directory
mkdir -p "$GLOBAL_SKILLS_DIR"
ensure_symlink "$GLOBAL_SKILLS_DIR/$SKILL_NAME" "$SKILL_DIR"

echo ""
echo "Done! Skill '$SKILL_NAME' is ready."
echo ""
echo "  Project skill : $SKILL_DIR"
echo "  Global symlink: $GLOBAL_SKILLS_DIR/$SKILL_NAME"
echo ""
echo "You can now use it in Claude Code with: /$SKILL_NAME <topic>"
echo ""
