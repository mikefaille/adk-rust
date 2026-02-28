#!/bin/bash
sed -i 's/user_id: "user1".to_string(),/user_id: "user1".to_string().into(),/g' adk-session/tests/database_tests.rs
sed -i 's/session_id: "session1".to_string(),/session_id: "session1".to_string().into(),/g' adk-session/tests/database_tests.rs
sed -i 's/session_id: Some("session1".to_string()),/session_id: Some("session1".to_string().into()),/g' adk-session/tests/database_tests.rs
sed -i 's/user_id: "user2".to_string(),/user_id: "user2".to_string().into(),/g' adk-session/tests/database_tests.rs
sed -i 's/session_id: "session2".to_string(),/session_id: "session2".to_string().into(),/g' adk-session/tests/database_tests.rs

sed -i 's/service.append_event("session1", event).await.unwrap();/service.append_event(\&"session1".to_string().into(), event).await.unwrap();/g' adk-session/tests/database_tests.rs
