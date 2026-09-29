# Conversation channels

The desktop bubble is face-to-face speech (`direct` / `bubble`). The app's private ChatWindow is online texting (`wechat` / `chat_window`), not external WeChat.

## Decision and delivery

- New proactive topics select a medium from intent, scene, presence, dialogue and recalled memory before composing text. The normal configured chat model performs this decision; no separate simple-judge configuration is required.
- An alive user/character conversation retains its channel. User input from another supported channel updates that choice. Closing or replacing a conversation releases the choice.
- Generation receives the existing channel style guide. Delivery verifies the generated channel and rejects incompatible queued messages; opening/closing ChatWindow and punctuation cannot change it.
- All generated behavior fields survive queueing, including work notices, music reactions and screen observations.
- Private delivery saves history as `wechat`, emits `chat:assistant_message`, and shows the existing message banner when the window is hidden. The private conversation preview and unread count update even when another conversation is selected.
- `send_chat_message` lets the character fulfill an explicit request to leave text in the app's private chat. Its instructions forbid unsolicited switching during an ongoing face-to-face topic.

## Checks

Run `npx tsc --noEmit`, `node --experimental-strip-types tests/chat-message-routing.test.mjs`, and `cargo test --manifest-path src-tauri/Cargo.toml --lib --offline -- send_chat_message_tool channel_tests delivery_channel_tests topic_transport_tests`.

Manual acceptance after restarting the updated app: ask from a desktop bubble to leave a message in private chat; verify history and notification, then read it in ChatWindow. Continue a face-to-face topic while opening ChatWindow and confirm the topic stays spoken. Continue a private topic with the window hidden and confirm subsequent proactive messages remain text. Actual model wording and live window delivery require this runtime check.
