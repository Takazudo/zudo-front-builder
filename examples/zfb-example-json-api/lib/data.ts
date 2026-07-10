export type ItemStatus = "ready" | "review" | "draft";

export type DemoItem = {
  id: string;
  name: string;
  category: string;
  status: ItemStatus;
  owner: string;
  summary: string;
  tags: string[];
  updatedAt: string;
};

export const ITEMS: readonly DemoItem[] = [
  {
    id: "item-001",
    name: "Atlas intake checklist",
    category: "Operations",
    status: "ready",
    owner: "Ops Desk",
    summary: "A neutral checklist for collecting request details before work is routed.",
    tags: ["intake", "handoff", "checklist"],
    updatedAt: "2026-01-08",
  },
  {
    id: "item-002",
    name: "Harbor schedule notes",
    category: "Planning",
    status: "review",
    owner: "Planning Desk",
    summary: "Short scheduling notes for a fictional quarterly coordination cycle.",
    tags: ["calendar", "milestones", "planning"],
    updatedAt: "2026-01-18",
  },
  {
    id: "item-003",
    name: "Pine vendor roster",
    category: "Procurement",
    status: "ready",
    owner: "Vendor Desk",
    summary: "A sample roster for comparing general service providers and renewal dates.",
    tags: ["vendors", "renewals", "contacts"],
    updatedAt: "2026-01-26",
  },
  {
    id: "item-004",
    name: "Lumen release brief",
    category: "Product",
    status: "draft",
    owner: "Product Desk",
    summary: "A concise release note template for a fictional internal tool update.",
    tags: ["release", "notes", "product"],
    updatedAt: "2026-02-02",
  },
  {
    id: "item-005",
    name: "Quartz support macros",
    category: "Support",
    status: "ready",
    owner: "Support Desk",
    summary: "Reusable support replies for common account, access, and routing questions.",
    tags: ["support", "macros", "response"],
    updatedAt: "2026-02-10",
  },
  {
    id: "item-006",
    name: "Ember risk register",
    category: "Compliance",
    status: "review",
    owner: "Risk Desk",
    summary: "A lightweight register for tracking generic operational risks and mitigations.",
    tags: ["risk", "review", "tracking"],
    updatedAt: "2026-02-19",
  },
  {
    id: "item-007",
    name: "Willow training plan",
    category: "People",
    status: "ready",
    owner: "People Desk",
    summary: "A neutral training plan for onboarding a small internal project group.",
    tags: ["training", "onboarding", "people"],
    updatedAt: "2026-02-24",
  },
  {
    id: "item-008",
    name: "Copper budget rollup",
    category: "Finance",
    status: "draft",
    owner: "Finance Desk",
    summary: "A fictional budget summary with placeholders for forecast and actual spend.",
    tags: ["budget", "forecast", "finance"],
    updatedAt: "2026-03-03",
  },
  {
    id: "item-009",
    name: "Northstar field log",
    category: "Research",
    status: "ready",
    owner: "Research Desk",
    summary: "A sample field-note structure for recording observations and follow-ups.",
    tags: ["research", "notes", "follow-up"],
    updatedAt: "2026-03-08",
  },
  {
    id: "item-010",
    name: "Papertrail archive map",
    category: "Records",
    status: "review",
    owner: "Records Desk",
    summary: "A map of fictional archive folders, retention labels, and cleanup owners.",
    tags: ["records", "archive", "cleanup"],
    updatedAt: "2026-03-15",
  },
  {
    id: "item-011",
    name: "Meadow feedback themes",
    category: "Research",
    status: "ready",
    owner: "Research Desk",
    summary: "A grouped list of neutral feedback themes from an imagined pilot survey.",
    tags: ["feedback", "themes", "survey"],
    updatedAt: "2026-03-20",
  },
  {
    id: "item-012",
    name: "Orbit launch timeline",
    category: "Product",
    status: "review",
    owner: "Product Desk",
    summary: "A compact launch timeline with sample checkpoints and review gates.",
    tags: ["launch", "timeline", "checkpoints"],
    updatedAt: "2026-03-27",
  },
  {
    id: "item-013",
    name: "Linen policy index",
    category: "Compliance",
    status: "ready",
    owner: "Policy Desk",
    summary: "An index of fictional internal policies and the teams that maintain them.",
    tags: ["policy", "index", "maintenance"],
    updatedAt: "2026-04-01",
  },
  {
    id: "item-014",
    name: "Pebble facility rounds",
    category: "Facilities",
    status: "draft",
    owner: "Facilities Desk",
    summary: "A simple walkthrough log for office supplies, rooms, and general repairs.",
    tags: ["facilities", "walkthrough", "log"],
    updatedAt: "2026-04-05",
  },
  {
    id: "item-015",
    name: "Cinder access review",
    category: "Security",
    status: "review",
    owner: "Access Desk",
    summary: "A fictional access-review list for checking role assignments and exceptions.",
    tags: ["access", "security", "review"],
    updatedAt: "2026-04-10",
  },
  {
    id: "item-016",
    name: "Bridge status rollup",
    category: "Operations",
    status: "ready",
    owner: "Ops Desk",
    summary: "A short cross-team status rollup for active work, blocked items, and owners.",
    tags: ["status", "rollup", "handoff"],
    updatedAt: "2026-04-16",
  },
  {
    id: "item-017",
    name: "Prism research digest",
    category: "Research",
    status: "ready",
    owner: "Research Desk",
    summary: "A digest format for summarizing recent findings and open questions.",
    tags: ["digest", "research", "questions"],
    updatedAt: "2026-04-21",
  },
  {
    id: "item-018",
    name: "Stone incident template",
    category: "Support",
    status: "review",
    owner: "Support Desk",
    summary: "A neutral template for incident notes, timeline entries, and action items.",
    tags: ["incident", "template", "actions"],
    updatedAt: "2026-04-26",
  },
  {
    id: "item-019",
    name: "Maple hiring pipeline",
    category: "People",
    status: "draft",
    owner: "People Desk",
    summary: "A fictional hiring tracker with stages, interview loops, and next steps.",
    tags: ["hiring", "pipeline", "people"],
    updatedAt: "2026-05-01",
  },
  {
    id: "item-020",
    name: "Nova partner FAQ",
    category: "Partnerships",
    status: "ready",
    owner: "Partner Desk",
    summary: "A compact FAQ for a sample partner onboarding and support process.",
    tags: ["faq", "partners", "onboarding"],
    updatedAt: "2026-05-06",
  },
  {
    id: "item-021",
    name: "Amber renewal calendar",
    category: "Procurement",
    status: "review",
    owner: "Vendor Desk",
    summary: "A renewal calendar for fictional contracts, reminders, and account owners.",
    tags: ["renewals", "calendar", "vendors"],
    updatedAt: "2026-05-12",
  },
  {
    id: "item-022",
    name: "Cedar inventory snapshot",
    category: "Facilities",
    status: "ready",
    owner: "Facilities Desk",
    summary: "A sample inventory snapshot for office equipment and reorder thresholds.",
    tags: ["inventory", "facilities", "snapshot"],
    updatedAt: "2026-05-17",
  },
  {
    id: "item-023",
    name: "Compass quality notes",
    category: "Quality",
    status: "draft",
    owner: "Quality Desk",
    summary: "A note set for tracking review criteria, defects, and acceptance decisions.",
    tags: ["quality", "review", "criteria"],
    updatedAt: "2026-05-23",
  },
  {
    id: "item-024",
    name: "Relay handbook update",
    category: "People",
    status: "ready",
    owner: "People Desk",
    summary: "A fictional handbook update log with owners, sections, and change notes.",
    tags: ["handbook", "updates", "people"],
    updatedAt: "2026-05-28",
  },
  {
    id: "item-025",
    name: "Summit retrospective log",
    category: "Planning",
    status: "review",
    owner: "Planning Desk",
    summary: "A retrospective log for collecting observations, decisions, and follow-ups.",
    tags: ["retro", "planning", "follow-up"],
    updatedAt: "2026-06-02",
  },
  {
    id: "item-026",
    name: "Tide onboarding queue",
    category: "Support",
    status: "ready",
    owner: "Support Desk",
    summary: "A queue view for sample account setup, verification, and welcome steps.",
    tags: ["onboarding", "queue", "support"],
    updatedAt: "2026-06-07",
  },
  {
    id: "item-027",
    name: "Granite audit trail",
    category: "Compliance",
    status: "draft",
    owner: "Policy Desk",
    summary: "A fictional audit trail for recording changed settings and reviewer notes.",
    tags: ["audit", "settings", "review"],
    updatedAt: "2026-06-12",
  },
  {
    id: "item-028",
    name: "Beacon content plan",
    category: "Content",
    status: "ready",
    owner: "Content Desk",
    summary: "A compact content plan with sample topics, channels, and publish windows.",
    tags: ["content", "planning", "channels"],
    updatedAt: "2026-06-18",
  },
  {
    id: "item-029",
    name: "Alloy service catalog",
    category: "Operations",
    status: "review",
    owner: "Ops Desk",
    summary: "A neutral catalog of internal services, request paths, and response targets.",
    tags: ["catalog", "services", "operations"],
    updatedAt: "2026-06-24",
  },
  {
    id: "item-030",
    name: "Fern meeting actions",
    category: "Planning",
    status: "ready",
    owner: "Planning Desk",
    summary: "A small action-item register for meetings, owners, and due windows.",
    tags: ["meetings", "actions", "owners"],
    updatedAt: "2026-06-30",
  },
];

const SEARCH_TEXT = new Map(
  ITEMS.map((item) => [
    item.id,
    [
      item.name,
      item.category,
      item.status,
      item.owner,
      item.summary,
      item.tags.join(" "),
      item.updatedAt,
    ]
      .join(" ")
      .toLowerCase(),
  ]),
);

export const ITEM_BY_ID = new Map(ITEMS.map((item) => [item.id, item]));

export function normalizeQuery(value: string | null): string {
  return (value ?? "").trim().replace(/\s+/g, " ").toLowerCase();
}

export function filterItems(query: string | null): DemoItem[] {
  const tokens = normalizeQuery(query).split(" ").filter(Boolean);
  if (tokens.length === 0) {
    return [...ITEMS];
  }
  return ITEMS.filter((item) => {
    const text = SEARCH_TEXT.get(item.id) ?? "";
    return tokens.every((token) => text.includes(token));
  });
}
