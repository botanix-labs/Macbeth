#[macro_export]
macro_rules! run_test {
    ($self:ident, $module:ident :: $scope:ident :: $test_name:ident $(, $arg:expr )* $(,)?) => {
        let suite = std::any::type_name::<Self>()
            .trim_start_matches(|c: char| !c.is_uppercase());
        let test = [suite, ": ", stringify!($module), "::", stringify!($scope), "::", stringify!($test_name)].concat();
        let test = test.as_str();

        let red = |text| ansi_term::Color::Red.bold().paint(text);
        let cyan = |text| ansi_term::Color::Cyan.bold().paint(text);
        let green = |text| ansi_term::Color::Green.bold().paint(text);
        let purple = |text| ansi_term::Color::Purple.bold().paint(text);

        let timer = tokio::time::sleep($self.timeout);
        tokio::pin!(timer);
        let started = std::time::Instant::now();
        let elapsed = || started.elapsed().as_millis();

        let test_type = if $self.global_context.dry_run {
            "DryRun"
        } else {
            "FullRun"
        };

        $self.create_context().await;

        tokio::select! {
            result = $module::$scope::$test_name($self $(, $arg )*) => match result {
                Ok(_) => {
                    info!("({}) {} {} ({}ms)", purple(test_type), cyan(test), green("\u{2713} PASSED"), elapsed());
                }
                Err(err) => {
                    error!("({}) {} {} ({}ms): {}", purple(test_type), red(test), red("\u{2718} FAILED"), elapsed(), err);
                    $self.outcomes.push(crate::suite::Outcome::Failed);
                }
            },

            _ = &mut timer => {
                error!("({}) {} {} ({}ms): timeout", purple(test_type), red(test), red("\u{2718} FAILED"), elapsed());
                $self.outcomes.push(crate::suite::Outcome::Failed);
            }
        }
    };
}

#[macro_export]
macro_rules! it_info_print {
    ($string_message:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::info!("({}) {:?}", label, $string_message);
    }};
    ($string_message:expr, $info1:expr, $info2:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::info!("({}) {} {:?} {:?}", label, $string_message, $info1, $info2);
    }};
    ($string_message:expr, $info:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::info!("({}) {} {:?}", label, $string_message, $info);
    }};
}

#[macro_export]
macro_rules! it_error_print {
    ($string_message:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::error!("({}) {:?}", label, $string_message);
    }};
    ($string_message:expr, $info1:expr, $info2:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::error!("({}) {} {:?} {:?}", label, $string_message, $info1, $info2);
    }};
    ($string_message:expr, $info:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::error!("({}) {} {:?}", label, $string_message, $info);
    }};
}

#[macro_export]
macro_rules! it_warn_print {
    ($string_message:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::warn!("({}) {:?}", label, $string_message);
    }};
    ($string_message:expr, $info1:expr, $info2:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::warn!("({}) {} {:?} {:?}", label, $string_message, $info1, $info2);
    }};
    ($string_message:expr, $info:expr) => {{
        let label = ansi_term::Color::Purple.bold().paint(">>>>>>>>>>> IT_SUITE <<<<<<<<<<<<");
        tracing::warn!("({}) {} {:?}", label, $string_message, $info);
    }};
}
