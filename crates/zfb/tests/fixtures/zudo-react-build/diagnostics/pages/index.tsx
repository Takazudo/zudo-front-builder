import Document from "../layouts/document";
import { SearchField } from "../components/search-field";

export default function Home() {
  return (
    <Document>
      <main>
        <SearchField />
      </main>
    </Document>
  );
}
