#!/bin/bash
sed -i 's/session.id(), "session1"/session.id().as_str(), "session1"/g' adk-session/tests/database_tests.rs
sed -i 's/session.user_id(), "user1"/session.user_id().as_str(), "user1"/g' adk-session/tests/database_tests.rs
sed -i 's/session_id: Some("session2".to_string()),/session_id: Some("session2".to_string().into()),/g' adk-session/tests/database_tests.rs
sed -i 's/user_id: "user1".to_string() }/user_id: "user1".to_string().into() }/g' adk-session/tests/database_tests.rs
sed -i 's/user_id: "user1".to_string(),/user_id: "user1".to_string().into(),/g' adk-session/examples/database_example.rs
sed -i 's/session_id: Some("session1".to_string()),/session_id: Some("session1".to_string().into()),/g' adk-session/examples/database_example.rs
sed -i 's/session_id: "session1".to_string(),/session_id: "session1".to_string().into(),/g' adk-session/examples/database_example.rs
sed -i 's/user_id: "user1".to_string() }/user_id: "user1".to_string().into() }/g' adk-session/examples/database_example.rs
sed -i 's/user_id: "user1".to_string() }/user_id: "user1".to_string().into() }/g' adk-session/examples/verify_database.rs
