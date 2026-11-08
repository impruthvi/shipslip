// PROTOTYPE (throwaway, branch prototype/app-shell).
// Three variants of the app shell with a checkout sidebar, Add project wizard
// and All projects map, switchable via ?variant=A|B|C|D (D = chosen mix, default). Fake data, no Rust.
// Run: cd app && npm run prototype   (opens /?prototype=app-shell)

import "./appShell.prototype.css";
import { params } from "./data.prototype";
import { Switcher } from "./parts.prototype";
import { VariantA } from "./VariantA.prototype";
import { VariantB } from "./VariantB.prototype";
import { VariantC } from "./VariantC.prototype";
import { VariantD } from "./VariantD.prototype";

const variants = [
  { key: "A", name: "Ledger · one-row map · one-screen wizard" },
  { key: "B", name: "Cards · grouped sidebar · stepped wizard" },
  { key: "C", name: "Master–detail · wizard sheet" },
  { key: "D", name: "Chosen mix" },
];

export default function AppShellPrototype() {
  const v = params.get("variant") ?? "D";
  return (
    <>
      {v === "A" && <VariantA />}
      {v === "B" && <VariantB />}
      {v === "C" && <VariantC />}
      {v === "D" && <VariantD />}
      <Switcher variants={variants} current={v} />
    </>
  );
}
