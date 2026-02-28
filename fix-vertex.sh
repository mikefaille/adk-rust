#!/bin/bash
sed -i 's/user_id: req.user_id.clone(),/user_id: req.user_id.clone().into(),/g' adk-session/src/vertex.rs
sed -i 's/user_id: req.user_id,/user_id: req.user_id.into(),/g' adk-session/src/vertex.rs
sed -i 's/payload.user_id != req.user_id/payload.user_id != req.user_id.as_str()/g' adk-session/src/vertex.rs
sed -i 's/session_id: req.session_id,/session_id: req.session_id.into(),/g' adk-session/src/vertex.rs

sed -i 's/async fn append_event(&self, session_id: &str, mut event: Event)/async fn append_event(\&self, session_id: \&SessionId, mut event: Event)/' adk-session/src/vertex.rs
sed -i 's/fn id(&self) -> &str {/fn id(\&self) -> \&SessionId {/g' adk-session/src/vertex.rs
sed -i 's/fn user_id(&self) -> &str {/fn user_id(\&self) -> \&UserId {/g' adk-session/src/vertex.rs

sed -i 's/user_id: String,/user_id: UserId,/g' adk-session/src/vertex.rs
sed -i 's/session_id: String,/session_id: SessionId,/g' adk-session/src/vertex.rs

sed -i 's/use adk_core::{AdkError/use adk_core::{AdkError, types::{SessionId, UserId}/g' adk-session/src/vertex.rs

sed -i 's/user_id: user_id.to_string()/user_id: user_id.to_string().into()/g' adk-session/src/vertex.rs
sed -i 's/scope.user_id == user_id/scope.user_id.as_str() == user_id/g' adk-session/src/vertex.rs
sed -i 's/payload.user_id != req.user_id.as_str()/payload.user_id != req.user_id/g' adk-session/src/vertex.rs

sed -i 's/            session_id,/            session_id: session_id.into(),/g' adk-session/src/vertex.rs
sed -i 's/                    session_id,/                    session_id: session_id.into(),/g' adk-session/src/vertex.rs
