export default {
  name: "cross-pipeline-plugin-alias",
  setup({ addAlias }) {
    addAlias("plugin:cross-pipeline-alias-raw", "./plugin-alias-raw.ts");
  },
};
