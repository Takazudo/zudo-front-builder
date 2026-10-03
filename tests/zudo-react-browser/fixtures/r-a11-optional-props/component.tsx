type OptionalRecord = {
  description?: string | null;
};

type OptionalProps = {
  locale?: string | null;
  currentSlug?: string | null;
  summary: { title: string; description?: string | null };
  records: Array<{
    id: string;
    description?: string | null;
    details: Array<{ label: string } & OptionalRecord>;
  }>;
};

function describe(record: OptionalRecord) {
  const present = Object.hasOwn(record, "description");
  return { present, value: present ? record.description : "omitted" };
}

export default function OptionalPropsProbe(props: OptionalProps) {
  const state = {
    locale: Object.hasOwn(props, "locale"),
    currentSlug: Object.hasOwn(props, "currentSlug"),
    summary: describe(props.summary),
    records: props.records.map((record) => ({
      id: record.id,
      ...describe(record),
      details: record.details.map((detail) => ({ label: detail.label, ...describe(detail) })),
    })),
  };

  return <output id="optional-props-state">{JSON.stringify(state)}</output>;
}
