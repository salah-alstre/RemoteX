import { useTranslation } from "react-i18next";
import { Toggle } from "./ui";
import type { PermissionKey, Permissions } from "../lib/types";

/** Standard permissions shown as individual switches. Elevated control is chosen as an access level instead. */
export const availablePermissions: PermissionKey[] = ["view", "mouse", "keyboard", "clipboard", "files"];

export function PermissionToggles({
  value,
  onChange,
  only,
}: {
  value: Permissions;
  onChange: (p: Permissions) => void;
  only?: Partial<Permissions>;
}) {
  const { t } = useTranslation();
  return (
    <div className="divide-y divide-line rounded-xl border border-line px-3">
      {availablePermissions
        .filter((k) => only === undefined || only[k])
        .map((k) => (
          <Toggle key={k} label={t(`perm.${k}`)} checked={value[k]} onChange={(v) => onChange({ ...value, [k]: v })} />
        ))}
      {/* Elevated control can only be switched off here once granted; it is granted through the access level. */}
      {value.elevated && <Toggle label={t("perm.elevated")} checked onChange={(v) => onChange({ ...value, elevated: v })} />}
    </div>
  );
}
