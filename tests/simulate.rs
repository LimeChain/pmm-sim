use pmm_sim;

#[cfg(tests)]
mod tests {
    use pmm_sim::CliArgs;

    #[test]
    fn test_cli_args() {
        let args = CliArgs::parse();
        assert!(args.verbose);
    }
}
