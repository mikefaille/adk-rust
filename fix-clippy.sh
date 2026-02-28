#!/bin/bash
sed -i 's/user_id: req.user_id.clone().into(),/user_id: req.user_id.clone(),/g' adk-session/src/vertex.rs
sed -i 's/user_id: req.user_id.into(),/user_id: req.user_id,/g' adk-session/src/vertex.rs
sed -i 's/session_id: req.session_id.into(),/session_id: req.session_id,/g' adk-session/src/vertex.rs
