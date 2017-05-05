error_chain!{
    errors {
        FailedToInitialize {}
        FailedToUnsubscribe {}
        FailedToPublish {}
        Timeout {}
        InternalError {}
    }

    foreign_links {
        Io(::std::io::Error);
    }
}
