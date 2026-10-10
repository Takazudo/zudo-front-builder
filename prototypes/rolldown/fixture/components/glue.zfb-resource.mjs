export const answer = 42;
// SSR imports this as code; the browser's locked loader copies the whole file.
export default "unused-during-SSR";
