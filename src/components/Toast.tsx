import { Show } from "solid-js";
import { Icon } from "./Icon";
import { setToastMsg, toastMsg } from "../store/app";

export function Toast() {
  return (
    <Show when={toastMsg()}>
      {(t) => (
        <div class="toast" role="status" data-tone={t().tone}>
          <Show when={t().tone === "error"}>
            <Icon name="alert" size={15} color="var(--status-crashed)" />
          </Show>
          <span class="selectable">{t().text}</span>
          <button class="icon-btn" style={{ width: "18px", height: "18px" }} onClick={() => setToastMsg(null)}>
            <Icon name="x" size={11} />
          </button>
        </div>
      )}
    </Show>
  );
}
