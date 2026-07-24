    use super::*;

    fn p(uri: &str) -> Option<String> {
        uri_to_path(&uri.parse::<Uri>().ok()?).map(|p| p.display().to_string())
    }

    #[test]
    fn uri_to_path_handles_empty_and_host_authorities() {
        // The common form: empty authority, leading `/` is the path.
        assert_eq!(
            p("file:///home/x/a.blsp").as_deref(),
            Some("/home/x/a.blsp")
        );
        // A host authority (WSL / remote) is dropped; the path stays absolute.
        assert_eq!(
            p("file://localhost/home/x/a.blsp").as_deref(),
            Some("/home/x/a.blsp")
        );
        // Percent-escapes in the path decode (a space here).
        assert_eq!(
            p("file:///home/a%20b/p.blsp").as_deref(),
            Some("/home/a b/p.blsp")
        );
    }

    #[test]
    fn path_to_uri_round_trips_through_uri_to_path() {
        for path in ["/home/x/a.blsp", "/home/a b/p.blsp", "/tmp/π/x.blsp"] {
            let uri = path_to_uri(path).expect("uri");
            assert_eq!(uri_to_path(&uri).unwrap().display().to_string(), path);
        }
    }
