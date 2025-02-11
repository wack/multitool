use clap::Args;

#[derive(Args, Clone)]
pub struct LoginSubcommand {
    /// The email of the account
    #[clap(long)]
    email: Option<String>,
    /// The password of the account
    #[clap(long)]
    password: Option<String>,
}

impl LoginSubcommand {
    /// Return the user's email, if provided via the CLI.
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    /// Return the user's password, if provided via the CLI.
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }
}
