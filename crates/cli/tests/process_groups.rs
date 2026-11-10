//! Process groups across two real `brood` runtimes (`std/proc/groups.blsp`): a member that
//! joins on one node is listed on the other, and leaves that list when its node goes.
//!
//! B joins after connecting, before either group server has polled the other in, so B's
//! member reaches A by the sync a new peer gets, not by the join's own broadcast; A's
//! member, joined before B existed, reaches B the same way. B then exits, and A must drop
//! B's member when the link goes down.

mod support;
use support::*;

#[test]
fn a_member_is_seen_across_nodes_and_dropped_with_its_node() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-groups-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    let wait_for = r#"
(defn wait-for (ready? tries)
  (cond (ready?) true (= tries 0) false else (do (proc/sleep 50) (wait-for ready? (dec tries)))))
"#;

    let node_a = format!(
        r#"
(node/start :a "127.0.0.1:{port_a}" "secret-test-cookie-16+")
(groups/start {{:interval 100}})
(def worker (spawn (receive (:stop :ok))))
(groups/join :workers worker)
{wait_for}
(when (not (wait-for (fn () (= 2 (count (groups/members :workers)))) 600))
  (throw "A never saw B's member"))
(io/puts "A-SEES-B")
(when (not (= 1 (count (groups/local-members :workers))))
  (throw "A counted B's member as local"))
(when (not (wait-for (fn () (= (list worker) (groups/members :workers))) 600))
  (throw "A kept B's member after B's node went"))
(io/puts "A-DROPPED-B")
(system/halt 0)
"#
    );

    let node_b = format!(
        r#"
(node/start :b "127.0.0.1:{port_b}" "secret-test-cookie-16+")
(groups/start {{:interval 100}})
(node/connect "a@127.0.0.1:{port_a}")
(groups/join :workers)
{wait_for}
(when (not (wait-for (fn () (= 2 (count (groups/members :workers)))) 600))
  (throw "B never saw A's member"))
(io/puts "B-SEES-A")
(proc/sleep 500)
(system/halt 0)
"#
    );

    let a = spawn_brood(&dir, "a.blsp", &node_a);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "b.blsp", &node_b);

    let b_out = b.wait_with_output().expect("node B finished");
    let a_out = a.wait_with_output().expect("node A finished");
    let a_err = String::from_utf8_lossy(&a_out.stderr);
    let _ = std::fs::remove_dir_all(&dir);

    let a_stdout = String::from_utf8_lossy(&a_out.stdout);
    let b_stdout = String::from_utf8_lossy(&b_out.stdout);
    let b_stderr = String::from_utf8_lossy(&b_out.stderr);
    assert!(
        b_out.status.success() && b_stdout.contains("B-SEES-A"),
        "node B failed.\n--- B stdout ---\n{b_stdout}\n--- B stderr ---\n{b_stderr}\n--- A stderr ---\n{a_err}"
    );
    assert!(
        a_out.status.success() && a_stdout.contains("A-SEES-B") && a_stdout.contains("A-DROPPED-B"),
        "node A failed.\n--- A stdout ---\n{a_stdout}\n--- A stderr ---\n{a_err}\n--- B stderr ---\n{b_stderr}"
    );
}
