#!/bin/bash
sed -i 's/\.bind(&req\.user_id)/\.bind(req\.user_id\.as_str())/g' adk-session/src/database.rs
sed -i 's/\.bind(&req\.session_id)/\.bind(req\.session_id\.as_str())/g' adk-session/src/database.rs
sed -i 's/\.bind(&session_id)/\.bind(session_id\.as_str())/g' adk-session/src/database.rs

sed -i 's/req\.session_id\.unwrap_or_else(|| Uuid::new_v4().to_string())/req.session_id.unwrap_or_else(|| Uuid::new_v4().to_string().into())/g' adk-session/src/database.rs

sed -i 's/user_id: String,/user_id: adk_core::types::UserId,/g' adk-session/src/database.rs
sed -i 's/session_id: String,/session_id: adk_core::types::SessionId,/g' adk-session/src/database.rs

sed -i 's/fn id(&self) -> &str {/fn id(\&self) -> \&adk_core::types::SessionId {/g' adk-session/src/database.rs
sed -i 's/fn user_id(&self) -> &str {/fn user_id(\&self) -> \&adk_core::types::UserId {/g' adk-session/src/database.rs

sed -i 's/async fn append_event(&self, session_id: &str, mut event: Event)/async fn append_event(\&self, session_id: \&adk_core::types::SessionId, mut event: Event)/' adk-session/src/database.rs
sed -i 's/\.bind(session_id)/\.bind(session_id.as_str())/g' adk-session/src/database.rs

sed -i 's/user_id: req.user_id.clone(),/user_id: req.user_id.clone(),/g' adk-session/src/database.rs
sed -i 's/session_id: row.get("session_id"),/session_id: adk_core::types::SessionId::from(row.get::<String, _>("session_id")),/g' adk-session/src/database.rs
