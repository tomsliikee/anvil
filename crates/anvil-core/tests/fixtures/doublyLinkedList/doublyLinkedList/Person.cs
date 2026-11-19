namespace doublyLinkedList
{
    internal class Person
    {
        private string firstName;
        private string lastName;
        private DateTime birthDay;
        private Sex sex;

        public Person(string firstName, string lastName, DateTime birthDay, Sex sex)
        {
            this.FirstName = firstName;
            this.LastName = lastName;
            this.BirthDay = birthDay;
            this.Sex = sex;
        }

        public string FirstName
        {
            get
            {
                return this.firstName;
            }

            private set
            {
                if (value == null)
                {
                    throw new ArgumentNullException(nameof(this.FirstName), $"This specified value {nameof(this.FirstName)} cannot be null");
                }

                if (value == string.Empty)
                {
                    throw new ArgumentException(nameof(this.FirstName), $"This specified value {nameof(this.FirstName)} cannot be empty");
                }

                this.firstName = value;
            }
        }

        public string LastName
        {
            get
            {
                return this.lastName;
            }

            private set
            {
                if (value == null)
                {
                    throw new ArgumentNullException(nameof(this.LastName), $"This specified value {nameof(this.LastName)} cannot be null");
                }

                if (value == string.Empty)
                {
                    throw new ArgumentException(nameof(this.LastName), $"This specified value {nameof(this.LastName)} cannot be empty");
                }

                this.lastName = value;
            }
        }

        public DateTime BirthDay
        {
            get
            {
                return this.BirthDay;
            }

            private set
            {
                if (value > DateTime.Now)
                {
                    throw new ArgumentOutOfRangeException(nameof(this.BirthDay), $"This specified value {this.BirthDay} cant be higher then today");
                }

                if (value < new DateTime(1900, 12, 20))
                {
                    throw new ArgumentOutOfRangeException(nameof(this.BirthDay), $"This specified value {this.BirthDay} cant be smaller then 1900, 12, 20");
                }

                this.birthDay = value;
            }

        }

        public Sex Sex
        {
            get
            {
                return this.sex;
            }

            private set
            {
                if (Enum.IsDefined(typeof(Sex), value))
                {
                    throw new ArgumentException(nameof(this.Sex), $"This specified value {this.Sex} can only be male or femal");
                }

                this.sex = value;
            }
        }
    }
}