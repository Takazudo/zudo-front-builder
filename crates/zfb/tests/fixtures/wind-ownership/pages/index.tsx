import FirstOwner from "../src/first-owner";
import SecondOwner from "../src/second-owner";
import SafelistOwner from "../src/safelist-owner";

export default function Home() {
  return (
    <html lang="en">
      <head>
        <title>Wind ownership fixture</title>
      </head>
      <body>
        <FirstOwner />
        <SecondOwner />
        <SafelistOwner />
      </body>
    </html>
  );
}
